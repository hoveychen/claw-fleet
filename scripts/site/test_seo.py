#!/usr/bin/env python3
"""Tests for the machine-readable half of the pages: head block, sitemap, robots."""
from pathlib import Path
import re
import sys
import unittest
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
import seo
import site_origin

ORIGIN = 'https://example.com/site'


def resolved(text):
    return site_origin.apply(text, ORIGIN)


def head(lang='en', **kw):
    args = dict(lang=lang, en_path='', zh_path='zh/', title='T', description='D',
                image='screenshots/current/work-en.png', image_size=(1288, 842),
                image_alt='A shot')
    args.update(kw)
    return resolved(seo.head(**args))


class HeadTests(unittest.TestCase):
    def test_each_language_canonicalises_to_itself(self):
        self.assertIn(f'<link rel="canonical" href="{ORIGIN}/">', head('en'))
        self.assertIn(f'<link rel="canonical" href="{ORIGIN}/zh/">', head('zh'))

    def test_both_languages_declare_the_same_alternate_set_including_themselves(self):
        # Google drops the whole hreflang cluster when a page omits a
        # self-reference, so this is the assertion that matters most.
        for lang in ('en', 'zh'):
            with self.subTest(lang=lang):
                out = head(lang)
                for hreflang, path in (('en', '/'), ('zh-CN', '/zh/'), ('x-default', '/')):
                    self.assertIn(f'<link rel="alternate" hreflang="{hreflang}" href="{ORIGIN}{path}">', out)

    def test_hreflang_and_og_url_are_absolute_after_substitution(self):
        out = head()
        self.assertNotIn(site_origin.TOKEN, out)
        self.assertIn(f'<meta property="og:url" content="{ORIGIN}/">', out)
        # A relative og:url or hreflang is what this replaced.
        self.assertNotIn('href="./', out)
        self.assertNotIn('content="./', out)

    def test_social_card_is_complete_on_both_networks(self):
        out = head()
        for fragment in ('og:site_name" content="Claw Fleet"', 'og:locale" content="en_US"',
                         'og:locale:alternate" content="zh_CN"', 'og:image:width" content="1288"',
                         'og:image:height" content="842"', 'og:image:alt" content="A shot"',
                         'twitter:card" content="summary_large_image"', 'twitter:title" content="T"',
                         'twitter:description" content="D"', 'twitter:image" content="'):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, out)

    def test_layout_markup_never_reaches_a_meta_attribute(self):
        out = head(title='A', description='one<br>two', image_alt='three<br>four')
        self.assertNotIn('&lt;br&gt;', out)
        self.assertIn('content="one two"', out)
        self.assertIn('content="three four"', out)

    def test_page_type_is_per_page(self):
        self.assertIn('og:type" content="article"', head(page_type='article'))
        self.assertIn('og:type" content="website"', head())

    def test_site_paths_are_root_relative(self):
        with self.assertRaises(ValueError):
            seo.url('/zh/')


class SitemapTests(unittest.TestCase):
    def setUp(self):
        self.xml = resolved(seo.sitemap([('', 'zh/'), ('benchmark.html', 'zh/benchmark.html')]))
        self.root = ET.fromstring(self.xml)

    def test_it_is_well_formed_and_lists_every_page_once(self):
        ns = {'s': 'http://www.sitemaps.org/schemas/sitemap/0.9'}
        locs = [e.text for e in self.root.findall('s:url/s:loc', ns)]
        self.assertEqual(locs, [f'{ORIGIN}/', f'{ORIGIN}/zh/',
                                f'{ORIGIN}/benchmark.html', f'{ORIGIN}/zh/benchmark.html'])
        self.assertEqual(len(locs), len(set(locs)))

    def test_every_entry_carries_the_full_alternate_set(self):
        ns = {'s': 'http://www.sitemaps.org/schemas/sitemap/0.9',
              'xhtml': 'http://www.w3.org/1999/xhtml'}
        for entry in self.root.findall('s:url', ns):
            langs = [l.get('hreflang') for l in entry.findall('xhtml:link', ns)]
            with self.subTest(loc=entry.find('s:loc', ns).text):
                self.assertEqual(langs, ['en', 'zh-CN', 'x-default'])

    def test_no_invented_metadata(self):
        # Fields we cannot keep truthful are worse than absent.
        for tag in ('lastmod', 'changefreq', 'priority'):
            self.assertNotIn(tag, self.xml)


class RobotsTests(unittest.TestCase):
    def test_it_allows_everything_and_points_at_the_sitemap(self):
        out = resolved(seo.robots())
        self.assertIn('User-agent: *', out)
        self.assertIn('Allow: /', out)
        self.assertIn(f'Sitemap: {ORIGIN}/sitemap.xml', out)
        self.assertFalse(re.search(r'^Disallow: /\s*$', out, re.M))


class GeneratedSiteTests(unittest.TestCase):
    """The committed docs/ tree is what gets published; check it, not just the helpers."""

    DOCS = Path(__file__).resolve().parents[2] / 'docs'

    def test_every_page_carries_a_canonical_and_a_self_referencing_hreflang(self):
        for name, canonical in (('index.html', '/'), ('zh/index.html', '/zh/'),
                                ('benchmark.html', '/benchmark.html'),
                                ('zh/benchmark.html', '/zh/benchmark.html')):
            with self.subTest(page=name):
                html = (self.DOCS / name).read_text()
                token = site_origin.TOKEN
                self.assertIn(f'<link rel="canonical" href="{token}{canonical}">', html)
                self.assertIn(f'hreflang="x-default" href="{token}/">'
                              if 'benchmark' not in name
                              else f'hreflang="x-default" href="{token}/benchmark.html">', html)
                self.assertIn(f'href="{token}{canonical}"', html)

    def test_the_sitemap_and_robots_are_generated_not_stale(self):
        from build import PAGE_PAIRS
        self.assertEqual((self.DOCS / 'sitemap.xml').read_text(), seo.sitemap(PAGE_PAIRS))
        self.assertEqual((self.DOCS / 'robots.txt').read_text(), seo.robots())


if __name__ == '__main__':
    unittest.main()
