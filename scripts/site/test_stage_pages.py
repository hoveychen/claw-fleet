#!/usr/bin/env python3
"""Tests for what GitHub Pages is allowed to publish."""
from pathlib import Path
import re
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import site_origin
import stage_pages

DOCS = Path(__file__).resolve().parents[2] / 'docs'
ORIGIN = 'https://hoveychen.github.io/claw-fleet'


class PayloadTests(unittest.TestCase):
    def setUp(self):
        self.names = stage_pages.payload(DOCS)

    def test_the_whole_site_is_published(self):
        for name in ('index.html', 'zh/index.html', 'benchmark.html', 'zh/benchmark.html',
                     'site.css', 'site.js', 'locale.js', 'benchmark.css', 'icon.png',
                     'sitemap.xml', 'robots.txt', 'downloads.json',
                     'screenshots/current/work-en.png', 'screenshots/current/mobile-zh.png'):
            with self.subTest(name=name):
                self.assertIn(name, self.names)

    def test_the_promo_player_stays_published(self):
        # Nothing on the landing page links to /player/, but the URL is already
        # shared; dropping it from the payload would 404 it.
        self.assertIn('player/index.html', self.names)
        self.assertTrue(any(n.startswith('player/footage/') for n in self.names))

    def test_internal_notes_and_ops_config_are_withheld(self):
        left = stage_pages.excluded(DOCS, self.names)
        for name in ('site-review.md', 'product-positioning.md', 'current-product-research.md',
                     'research/consumer-site/desktop.md', 'deploy/fleet-site-update.service',
                     'deploy/fleet-selfhost.nginx.conf', 'architecture/fleet-cloud-api-spike.md'):
            with self.subTest(name=name):
                self.assertIn(name, left)
                self.assertNotIn(name, self.names)

    def test_a_verification_file_dropped_in_the_root_gets_published(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'zh').mkdir()
            for name in ('index.html', 'zh/index.html', 'site.css', 'site.js', 'locale.js'):
                (root / name).write_text('')
            (root / 'google1234abcd.html').write_text('google-site-verification')
            (root / 'baidu_verify_codeva-xyz.html').write_text('xyz')
            (root / 'scratch-notes.txt').write_text('not part of the site')
            names = stage_pages.payload(root)
            self.assertIn('google1234abcd.html', names)
            self.assertIn('baidu_verify_codeva-xyz.html', names)
            # The whitelist still exists: an unrelated file stays unpublished.
            self.assertNotIn('scratch-notes.txt', names)

    def test_nothing_is_published_twice(self):
        self.assertEqual(len(self.names), len(set(self.names)))

    def test_payload_and_withheld_together_account_for_every_file(self):
        on_disk = {str(p.relative_to(DOCS)) for p in DOCS.rglob('*') if p.is_file()}
        # docs/releases is a mirror-only staging area and may not exist here.
        accounted = set(self.names) | set(stage_pages.excluded(DOCS, self.names))
        self.assertEqual(accounted, on_disk)


class StageTests(unittest.TestCase):
    def test_staging_resolves_the_origin_and_leaves_no_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / 'pages'
            stage_pages.stage(DOCS, out, ORIGIN)
            html = (out / 'index.html').read_text()
            self.assertNotIn(site_origin.TOKEN, html)
            self.assertIn(f'<link rel="canonical" href="{ORIGIN}/">', html)
            self.assertIn(f'Sitemap: {ORIGIN}/sitemap.xml', (out / 'robots.txt').read_text())
            self.assertIn(f'<loc>{ORIGIN}/zh/</loc>', (out / 'sitemap.xml').read_text())
            # assert_no_token() inside stage() is the real guard; prove it holds
            # across the whole tree, not just the page we sampled.
            site_origin.assert_no_token(out)
            # Binary assets are copied, not rewritten.
            self.assertEqual((out / 'icon.png').read_bytes(), (DOCS / 'icon.png').read_bytes())

    def test_the_published_version_comes_from_the_manifest_pages_just_built(self):
        import json
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / 'pages'
            expected = json.loads((DOCS / 'downloads.json').read_text())['version'].lstrip('v')
            stage_pages.stage(DOCS, out, ORIGIN)
            html = (out / 'index.html').read_text()
            self.assertIn(f'"softwareVersion":"{expected}"', html)
            self.assertNotIn(site_origin.VERSION_TOKEN, html)

    def test_an_explicit_version_overrides_the_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / 'pages'
            stage_pages.stage(DOCS, out, ORIGIN, 'v9.9.9')
            self.assertIn('"softwareVersion":"9.9.9"', (out / 'index.html').read_text())

    def test_staging_without_a_manifest_fails_instead_of_publishing_a_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / 'site'
            shutil.copytree(DOCS, root, ignore=shutil.ignore_patterns('releases'))
            (root / 'downloads.json').unlink()
            with self.assertRaises(FileNotFoundError):
                stage_pages.stage(root, Path(tmp) / 'out', ORIGIN)

    def test_the_committed_pages_carry_a_token_not_a_frozen_version(self):
        # A version written at build time is wrong on every later release.
        self.assertIn(site_origin.VERSION_TOKEN, (DOCS / 'index.html').read_text())

    def test_the_workflow_uploads_the_staging_directory(self):
        # A whitelist nobody uploads is decoration.
        workflow = (Path(__file__).resolve().parents[2] / '.github/workflows/pages.yml').read_text()
        self.assertIn('scripts/site/stage_pages.py', workflow)
        self.assertRegex(workflow, r'path:\s*\$\{\{\s*runner\.temp\s*\}\}/pages')
        self.assertFalse(re.search(r'^\s*path:\s*docs\s*$', workflow, re.M))


if __name__ == '__main__':
    unittest.main()
