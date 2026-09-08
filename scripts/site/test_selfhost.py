"""Exercise activation boundaries without touching a live server."""
import copy
import hashlib
import http.client
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import distribute
import selfhost
from test_distribute import fixture


class SelfhostTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.old = self.root / 'deployments' / 'initial'
        self.release, self.body = fixture()
        self.previous = copy.deepcopy(self.release)
        self.previous['tag_name'] = 'v2.5.0'
        self.previous_body = b'previous immutable package\n'
        for asset in self.previous['assets']:
            asset['browser_download_url'] = asset['browser_download_url'].replace('v2.6.0', 'v2.5.0')
            asset['size'] = len(self.previous_body)
            asset['digest'] = 'sha256:' + hashlib.sha256(self.previous_body).hexdigest()
        with patch.object(distribute.urllib.request, 'urlopen', side_effect=lambda *a, **k: io.BytesIO(self.previous_body)):
            distribute.prepare(self.previous, self.old, 'https://dl.example.com', provider='Shenzhen')
        (self.root / 'current').symlink_to(self.old)

    def test_no_new_release_does_not_download_or_change_site(self):
        with patch.object(distribute.urllib.request, 'urlopen') as network:
            self.assertFalse(selfhost.sync(self.root, 'https://dl.example.com', self.previous))
            network.assert_not_called()
        self.assertEqual((self.root / 'current').resolve(), self.old)
        self.assertEqual(len(list((self.root / 'deployments').iterdir())), 1)

    def test_complete_release_switches_once_and_keeps_history_and_site(self):
        def download(*args, **kwargs):
            self.assertEqual((self.root / 'current').resolve(), self.old)
            return io.BytesIO(self.body)
        with patch.object(distribute.urllib.request, 'urlopen', side_effect=download):
            self.assertTrue(selfhost.sync(self.root, 'https://dl.example.com', self.release))
        live = (self.root / 'current').resolve()
        self.assertNotEqual(live, self.old)
        self.assertEqual(live.stat().st_mode & 0o777, 0o755)
        manifest = json.loads((live / 'downloads.json').read_text())
        self.assertEqual(manifest['version'], 'v2.6.0')
        self.assertEqual(manifest['china']['provider'], 'Shenzhen')
        # The site is carried over from the previous deployment as-is, with one
        # exception: the version its structured data claims is retargeted to the
        # release now being served. Otherwise the mirror would keep advertising
        # 2.5.0 while handing out 2.6.0 downloads.
        for page in ['index.html', 'zh/index.html', 'locale.js']:
            carried = (live / page).read_text()
            previous = (self.old / page).read_text()
            self.assertEqual(carried.replace('"softwareVersion":"2.6.0"', '"softwareVersion":"2.5.0"'),
                             previous)
        self.assertIn('"softwareVersion":"2.6.0"', (live / 'index.html').read_text())
        self.assertIn('"softwareVersion":"2.6.0"', (live / 'zh/index.html').read_text())
        for name in distribute.REQUIRED:
            old = self.old / 'releases/v2.5.0' / name
            retained = live / 'releases/v2.5.0' / name
            self.assertEqual(old.stat().st_ino, retained.stat().st_ino)
            self.assertEqual(retained.read_bytes(), self.previous_body)
            self.assertEqual((live / 'releases/v2.6.0' / name).read_bytes(), self.body)

    def test_corrupt_package_never_replaces_current(self):
        before = (self.old / 'downloads.json').read_bytes()
        # Full-length wrong bytes: this must exhaust the download retries and
        # fail, not look like a truncated transfer worth resuming.
        corrupt = b'x' * len(self.body)
        with patch.object(distribute.time, 'sleep', lambda *_: None), \
             patch.object(distribute.urllib.request, 'urlopen',
                          side_effect=lambda *a, **k: io.BytesIO(corrupt)):
            with self.assertRaises(ValueError):
                selfhost.sync(self.root, 'https://dl.example.com', self.release)
        self.assertEqual((self.root / 'current').resolve(), self.old)
        self.assertEqual((self.old / 'downloads.json').read_bytes(), before)
        # The stage is kept on purpose so the next round resumes into it; what
        # matters is that nothing unverified became live.
        staged = list((self.root / 'deployments').glob('.stage-v2.6.0-*'))
        self.assertEqual(len(staged), 1)
        self.assertFalse((staged[0] / 'downloads.json').exists())

    def test_a_killed_round_is_resumed_instead_of_restarted(self):
        # The mirror's route needs hours for the asset set and no unattended
        # round survives that long, so bytes have to accumulate across rounds.
        deployments = self.root / 'deployments'
        delivered = []

        def dies_after_two_assets(*args, **kwargs):
            if len(delivered) >= 2:
                raise http.client.RemoteDisconnected('Remote end closed connection without response')
            delivered.append('ok')
            return io.BytesIO(self.body)

        with patch.object(distribute.time, 'sleep', lambda *_: None), \
             patch.object(distribute.urllib.request, 'urlopen', side_effect=dies_after_two_assets):
            with self.assertRaises(OSError):
                selfhost.sync(self.root, 'https://dl.example.com', self.release)
        stage = list(deployments.glob('.stage-v2.6.0-*'))[0]
        kept = sorted(p.name for p in (stage / 'releases/v2.6.0').glob('*') if p.suffix != '.partial')
        self.assertEqual(len(kept), 2, 'the killed round must leave its verified assets behind')

        second = []

        def healthy(*args, **kwargs):
            second.append(args[0].full_url)
            return io.BytesIO(self.body)

        with patch.object(distribute.urllib.request, 'urlopen', side_effect=healthy):
            self.assertTrue(selfhost.sync(self.root, 'https://dl.example.com', self.release))
        # Only the two that were still missing were fetched again.
        self.assertEqual(len(second), len(distribute.REQUIRED) - 2)
        live = (self.root / 'current').resolve()
        self.assertEqual(json.loads((live / 'downloads.json').read_text())['version'], 'v2.6.0')
        # Reused the same stage rather than staging the set a second time, and
        # swept it away once activated.
        self.assertEqual(list(deployments.glob('.stage-*')), [])
        for name in distribute.REQUIRED:
            self.assertEqual((live / 'releases/v2.6.0' / name).read_bytes(), self.body)

    def test_the_fullest_stage_wins_not_the_newest(self):
        # A round that just started has the newest mtime and almost nothing in
        # it; the round killed before it may hold most of the asset set.
        deployments = self.root / 'deployments'
        fuller = deployments / '.stage-v2.6.0-killed'
        (fuller / 'releases/v2.6.0').mkdir(parents=True)
        for name in sorted(distribute.REQUIRED)[:3]:
            (fuller / 'releases/v2.6.0' / name).write_bytes(self.body)
        newer = deployments / '.stage-v2.6.0-juststarted'
        (newer / 'releases/v2.6.0').mkdir(parents=True)
        (newer / 'releases/v2.6.0/claw-fleet-macos.pkg.partial').write_bytes(b'x')
        self.assertGreater(newer.stat().st_mtime, fuller.stat().st_mtime)
        self.assertEqual(selfhost.staging_directory(deployments, 'v2.6.0'), fuller)

    def test_a_stage_for_another_tag_is_not_reused(self):
        deployments = self.root / 'deployments'
        stale = deployments / '.stage-v2.5.0-deadbeef'
        (stale / 'releases/v2.5.0').mkdir(parents=True)
        reused = selfhost.staging_directory(deployments, 'v2.6.0')
        self.assertNotEqual(reused, stale)
        self.assertTrue(stale.is_dir())

    def test_a_bad_staged_asset_is_discarded_and_refetched(self):
        # A round killed mid-write can leave a truncated file that is not a
        # .partial; trusting it would wedge every later round identically.
        deployments = self.root / 'deployments'
        stage = deployments / '.stage-v2.6.0-abandoned'
        assets = stage / 'releases/v2.6.0'
        assets.mkdir(parents=True)
        for name in distribute.REQUIRED:
            (assets / name).write_bytes(b'half-written junk')
        with patch.object(distribute.time, 'sleep', lambda *_: None), \
             patch.object(distribute.urllib.request, 'urlopen', side_effect=lambda *a, **k: io.BytesIO(self.body)):
            self.assertTrue(selfhost.sync(self.root, 'https://dl.example.com', self.release))
        live = (self.root / 'current').resolve()
        for name in distribute.REQUIRED:
            self.assertEqual((live / 'releases/v2.6.0' / name).read_bytes(), self.body)

    def test_incomplete_release_does_not_stage_or_activate(self):
        release = self.release | {'assets': self.release['assets'][:-1]}
        with self.assertRaises(ValueError):
            selfhost.sync(self.root, 'https://dl.example.com', release)
        self.assertEqual((self.root / 'current').resolve(), self.old)
        self.assertEqual(list((self.root / 'deployments').iterdir()), [self.old])

    def test_changed_immutable_release_is_rejected(self):
        changed = copy.deepcopy(self.previous)
        changed['assets'][0]['digest'] = 'sha256:' + '0' * 64
        with self.assertRaisesRegex(ValueError, 'immutable'):
            selfhost.sync(self.root, 'https://dl.example.com', changed, rebuild_current=True)
        self.assertEqual((self.root / 'current').resolve(), self.old)

    def test_rebuild_verifies_existing_packages_without_rewriting_them(self):
        checksums = (self.old / 'releases/v2.5.0/SHA256SUMS').read_bytes()
        with patch.object(distribute.urllib.request, 'urlopen') as network:
            self.assertTrue(selfhost.sync(self.root, 'https://dl.example.com', self.previous, rebuild_current=True))
            network.assert_not_called()
        live = (self.root / 'current').resolve()
        self.assertNotEqual(live, self.old)
        self.assertEqual((self.old / 'releases/v2.5.0/SHA256SUMS').read_bytes(), checksums)

    def test_older_upstream_is_not_a_downgrade(self):
        older = copy.deepcopy(self.previous)
        older['tag_name'] = 'v2.4.0'
        for asset in older['assets']:
            asset['browser_download_url'] = asset['browser_download_url'].replace('v2.5.0', 'v2.4.0')
        self.assertFalse(selfhost.sync(self.root, 'https://dl.example.com', older))
        self.assertEqual((self.root / 'current').resolve(), self.old)


if __name__ == '__main__':
    unittest.main()
