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
