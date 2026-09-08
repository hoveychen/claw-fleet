#!/usr/bin/env python3
"""Tests for the China mirror's site tree."""
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mirror_site
import site_origin

DOCS = Path(__file__).resolve().parents[2] / 'docs'
ORIGIN = 'https://fleet.eternizedlab.com'
VERSION = 'v2.7.0'


class IcpFooterTests(unittest.TestCase):
    def test_it_goes_inside_an_existing_footer(self):
        out = mirror_site.add_icp_footer('<body><footer><p>x</p></footer></body>')
        self.assertIn(mirror_site.ICP_NUMBER, out)
        self.assertLess(out.index(mirror_site.ICP_NUMBER), out.index('</footer>'))

    def test_a_page_without_a_footer_gets_one(self):
        # 404.html has no footer of its own and is served like any other page.
        out = mirror_site.add_icp_footer('<body><main>x</main></body>')
        self.assertIn('<footer', out)
        self.assertIn(mirror_site.ICP_NUMBER, out)
        self.assertLess(out.index(mirror_site.ICP_NUMBER), out.index('</body>'))

    def test_it_is_not_added_twice(self):
        once = mirror_site.add_icp_footer('<body><footer></footer></body>')
        self.assertEqual(mirror_site.add_icp_footer(once), once)


class BuildTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out = Path(self.tmp.name) / 'mirror'
        self.names = mirror_site.build(DOCS, self.out, ORIGIN, VERSION)

    def tearDown(self):
        self.tmp.cleanup()

    def test_every_page_carries_the_filing_number(self):
        pages = [n for n in self.names if n.endswith('.html')]
        self.assertGreaterEqual(len(pages), 13)  # 6 en + 6 zh + 404
        for name in pages:
            with self.subTest(page=name):
                self.assertIn(mirror_site.ICP_NUMBER, (self.out / name).read_text())

    def test_the_mirror_is_its_own_canonical(self):
        html = (self.out / 'index.html').read_text()
        self.assertIn(f'<link rel="canonical" href="{ORIGIN}/">', html)
        self.assertNotIn('hoveychen.github.io', html)
        self.assertNotIn(site_origin.TOKEN, html)
        self.assertIn('"softwareVersion":"2.7.0"', html)

    def test_the_mirrors_own_manifest_and_packages_are_left_alone(self):
        # They are the mirror's: its verified China assets and its own
        # downloads.json, which the Pages one would replace with GitHub URLs.
        self.assertNotIn('downloads.json', self.names)
        self.assertFalse((self.out / 'downloads.json').exists())
        self.assertFalse((self.out / 'releases').exists())

    def test_crawler_entry_points_are_included(self):
        for name in ('sitemap.xml', 'robots.txt', '404.html'):
            with self.subTest(name=name):
                self.assertIn(name, self.names)
        self.assertIn(f'Sitemap: {ORIGIN}/sitemap.xml', (self.out / 'robots.txt').read_text())

    def test_a_missing_footer_hook_is_an_error_not_a_silent_drop(self):
        # If the markup ever stops having a place to put the notice, the deploy
        # has to fail: a page that quietly ships without it looks fine.
        with tempfile.TemporaryDirectory() as tmp:
            docs = Path(tmp) / 'docs'
            (docs / 'zh').mkdir(parents=True)
            for name in ('site.css', 'site.js', 'locale.js'):
                (docs / name).write_text('')
            (docs / 'index.html').write_text('<html><p>no footer, no body close')
            (docs / 'zh/index.html').write_text('<html><footer></footer></html>')
            with self.assertRaises(ValueError) as caught:
                mirror_site.build(docs, Path(tmp) / 'out', ORIGIN, VERSION)
            self.assertIn('ICP footer missing', str(caught.exception))

    def test_the_mirrors_sitemap_gets_the_same_real_dates_pages_gets(self):
        # Without this the mirror would be the copy with no lastmod at all,
        # because the committed sitemap deliberately carries none.
        body = (self.out / 'sitemap.xml').read_text()
        self.assertEqual(body.count('<lastmod>'), body.count('<loc>'))
        self.assertRegex(body, r'<lastmod>\d{4}-\d{2}-\d{2}</lastmod>')
        # And the checkout it was built from is left untouched.
        self.assertNotIn('<lastmod>', (DOCS / 'sitemap.xml').read_text())

    def test_binaries_are_copied_byte_for_byte(self):
        self.assertEqual((self.out / 'icon.png').read_bytes(), (DOCS / 'icon.png').read_bytes())
        webp = next(n for n in self.names if n.endswith('.webp'))
        self.assertEqual((self.out / webp).read_bytes(), (DOCS / webp).read_bytes())


if __name__ == '__main__':
    unittest.main()
