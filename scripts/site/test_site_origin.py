#!/usr/bin/env python3
"""Tests for the publish-time site origin substitution."""
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import site_origin as so


class ValidateTests(unittest.TestCase):
    def test_accepts_a_bare_host_and_a_path_prefix(self):
        # Pages serves from a sub-path, the mirror from a bare host.
        self.assertEqual(so.validate_origin('https://fleet.eternizedlab.com'), 'https://fleet.eternizedlab.com')
        self.assertEqual(so.validate_origin('https://hoveychen.github.io/claw-fleet/'),
                         'https://hoveychen.github.io/claw-fleet')

    def test_rejects_origins_that_cannot_be_canonical(self):
        for value in ('http://example.com', 'https://u:p@example.com', 'https://example.com/?a=1',
                      'https://example.com/#x', 'https://example.com/../bad', 'example.com', ''):
            with self.subTest(value=value), self.assertRaises(ValueError):
                so.validate_origin(value)


class VersionTests(unittest.TestCase):
    def test_accepts_a_tag_with_or_without_the_v(self):
        self.assertEqual(so.validate_version('v2.7.0'), '2.7.0')
        self.assertEqual(so.validate_version('2.7.0'), '2.7.0')

    def test_rejects_anything_that_is_not_a_release_version(self):
        for value in ('latest', '2.7', 'v2.7.0-rc1', '', None, ' 2.7.0'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                so.validate_version(value)

    def test_an_already_resolved_version_is_retargeted(self):
        # The mirror re-publishes a site built for an older release.
        published = '<script>{"softwareVersion":"2.5.0","name":"Claw Fleet"}</script>'
        self.assertIn('"softwareVersion":"2.6.0"',
                      so.apply(published, 'https://example.com', 'v2.6.0'))

    def test_a_version_nobody_supplied_is_left_as_a_token_to_be_caught(self):
        out = so.apply(f'<x>{so.VERSION_TOKEN}</x>', 'https://example.com')
        self.assertIn(so.VERSION_TOKEN, out)

    def test_both_tokens_are_caught_by_the_publish_guard(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'a.html').write_text(f'<x>{so.VERSION_TOKEN}</x>')
            with self.assertRaises(ValueError) as caught:
                so.assert_no_token(root)
            self.assertIn(so.VERSION_TOKEN, str(caught.exception))

    def test_a_page_stating_the_wrong_version_is_an_error(self):
        # Guards the retarget regex: a markup change that made it silently
        # no-op would otherwise restore the stale number unnoticed.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'a.html').write_text('{"softwareVersion":"2.5.0"}')
            with self.assertRaises(ValueError):
                so.assert_version(root, 'v2.6.0')
            (root / 'a.html').write_text('{"softwareVersion":"2.6.0"}')
            so.assert_version(root, 'v2.6.0')
            # A page that states no version at all is not a violation.
            (root / 'b.html').write_text('<p>no version here</p>')
            so.assert_version(root, 'v2.6.0')


class ApplyTests(unittest.TestCase):
    def test_every_occurrence_in_a_document_is_replaced(self):
        text = f'<link rel="canonical" href="{so.TOKEN}/index.html"><meta content="{so.TOKEN}/icon.png">'
        out = so.apply(text, 'https://fleet.eternizedlab.com')
        self.assertNotIn(so.TOKEN, out)
        self.assertEqual(out.count('https://fleet.eternizedlab.com/'), 2)

    def test_tree_rewrite_touches_text_and_leaves_binaries_alone(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'zh').mkdir()
            (root / 'index.html').write_text(f'<a href="{so.TOKEN}/x">x</a>')
            (root / 'zh/index.html').write_text(f'<a href="{so.TOKEN}/y">y</a>')
            (root / 'sitemap.xml').write_text(f'<loc>{so.TOKEN}/</loc>')
            (root / 'site.css').write_text('body{color:red}')  # no token, must stay untouched
            (root / 'icon.png').write_bytes(so.TOKEN.encode() + b'\x89PNG')
            changed = so.apply_tree(root, 'https://example.com/site')
            self.assertEqual({p.name for p in changed}, {'index.html', 'index.html', 'sitemap.xml'})
            self.assertEqual(len(changed), 3)
            self.assertIn('https://example.com/site/x', (root / 'index.html').read_text())
            self.assertIn('https://example.com/site/y', (root / 'zh/index.html').read_text())
            # A binary that happens to contain the byte sequence is not a URL.
            self.assertTrue((root / 'icon.png').read_bytes().startswith(so.TOKEN.encode()))
            so.assert_no_token(root)

    def test_a_leftover_token_is_an_error_not_a_silent_publish(self):
        # A page with an unsubstituted token still renders; only the
        # machine-readable half is wrong, which is why it has to raise.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'index.html').write_text(f'<link rel="canonical" href="{so.TOKEN}/">')
            with self.assertRaises(ValueError):
                so.assert_no_token(root)

    def test_token_references_are_recognised(self):
        self.assertTrue(so.contains_token(f'{so.TOKEN}/index.html'))
        self.assertFalse(so.contains_token('./index.html'))


if __name__ == '__main__':
    unittest.main()
