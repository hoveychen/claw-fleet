"""Exercise activation boundaries without touching a live server."""
import copy
import hashlib
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
        for page in ['index.html', 'zh/index.html', 'locale.js']:
            self.assertEqual((live / page).read_bytes(), (self.old / page).read_bytes())
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
        self.assertEqual(list((self.root / 'deployments').iterdir()), [self.old])

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
