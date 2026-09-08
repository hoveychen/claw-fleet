#!/usr/bin/env python3
"""Tests for the machine-readable half of the pages: head block, sitemap, robots."""
from pathlib import Path
import json
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

    def test_ownership_proofs_only_appear_where_asked_for(self):
        # A proof is checked at the URL you submitted; repeating it on every
        # page is noise, and emitting an empty one before 老板 supplies codes
        # would be a meta tag with no content.
        original = seo.VERIFICATION
        try:
            seo.VERIFICATION = {'google-site-verification': 'abc', 'baidu-site-verification': 'def'}
            with_proof = head(verify_ownership=True)
            self.assertIn('<meta name="google-site-verification" content="abc">', with_proof)
            self.assertIn('<meta name="baidu-site-verification" content="def">', with_proof)
            self.assertNotIn('site-verification', head())
        finally:
            seo.VERIFICATION = original
        self.assertNotIn('site-verification', head(verify_ownership=True))

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


class JsonLdTests(unittest.TestCase):
    def parse(self, block):
        raw = re.fullmatch(r'<script type="application/ld\+json">(.*)</script>', block, re.S).group(1)
        self.assertNotIn('</', raw)  # would close the script element early
        return json.loads(raw.replace('<\\/', '</'))

    def test_the_graph_is_valid_json_and_escapes_closing_tags(self):
        doc = self.parse(resolved(seo.graph([seo.publisher_node(),
                                             seo.faq_node([('q</b>', 'a')], '')])))
        self.assertEqual(doc['@context'], 'https://schema.org')
        self.assertEqual([n['@type'] for n in doc['@graph']], ['Organization', 'FAQPage'])

    def test_nodes_cross_reference_by_id_and_resolve_to_absolute_urls(self):
        doc = self.parse(resolved(seo.graph([
            seo.publisher_node(),
            seo.website_node('en', 'D'),
            seo.software_node('en', 'D', ['screenshots/current/work-en.png'], ''),
        ])))
        nodes = {n['@type']: n for n in doc['@graph']}
        self.assertEqual(nodes['SoftwareApplication']['publisher']['@id'], nodes['Organization']['@id'])
        self.assertTrue(nodes['Organization']['@id'].startswith(ORIGIN))
        self.assertEqual(nodes['SoftwareApplication']['screenshot'],
                         [f'{ORIGIN}/screenshots/current/work-en.png'])

    def test_the_app_is_declared_free_without_inventing_a_rating(self):
        app = self.parse(resolved(seo.graph([seo.software_node('en', 'D', [], '')])))['@graph'][0]
        self.assertEqual(app['offers']['price'], '0')
        self.assertTrue(app['isAccessibleForFree'])
        for invented in ('aggregateRating', 'ratingValue', 'reviewCount'):
            self.assertNotIn(invented, app)

    def test_faq_entries_mirror_the_rendered_pairs_as_plain_text(self):
        faq = self.parse(resolved(seo.graph([seo.faq_node([('Q?', 'one<br>two')], 'zh/')])))['@graph'][0]
        self.assertEqual(faq['mainEntity'][0]['name'], 'Q?')
        self.assertEqual(faq['mainEntity'][0]['acceptedAnswer']['text'], 'one two')
        self.assertEqual(faq['@id'], f'{ORIGIN}/zh/#faq')

    def test_breadcrumb_positions_start_at_one(self):
        crumb = self.parse(resolved(seo.graph([
            seo.breadcrumb_node([('Home', ''), ('Report', 'benchmark.html')], 'benchmark.html'),
        ])))['@graph'][0]
        self.assertEqual([(i['position'], i['name'], i['item']) for i in crumb['itemListElement']],
                         [(1, 'Home', f'{ORIGIN}/'), (2, 'Report', f'{ORIGIN}/benchmark.html')])


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

    def test_every_page_ships_a_parseable_graph_with_the_right_node_types(self):
        expected = {
            'index.html': ['Organization', 'WebSite', 'SoftwareApplication', 'FAQPage'],
            'zh/index.html': ['Organization', 'WebSite', 'SoftwareApplication', 'FAQPage'],
            'benchmark.html': ['Organization', 'BreadcrumbList', 'FAQPage'],
            'zh/benchmark.html': ['Organization', 'BreadcrumbList', 'FAQPage'],
        }
        for name, types in expected.items():
            with self.subTest(page=name):
                html = (self.DOCS / name).read_text()
                raw = re.search(r'<script type="application/ld\+json">(.*?)</script>', html, re.S).group(1)
                doc = json.loads(raw.replace('<\\/', '</'))
                self.assertEqual([n['@type'] for n in doc['@graph']], types)

    def test_the_faq_markup_matches_the_faq_the_page_renders(self):
        # Structured data that contradicts the visible page is a manual action,
        # not an optimisation.
        for name in ('index.html', 'zh/index.html'):
            with self.subTest(page=name):
                html = (self.DOCS / name).read_text()
                rendered = [seo.plain(q) for q in re.findall(r'<summary>(.*?)<span', html)]
                raw = re.search(r'<script type="application/ld\+json">(.*?)</script>', html, re.S).group(1)
                doc = json.loads(raw.replace('<\\/', '</'))
                faq = next(n for n in doc['@graph'] if n['@type'] == 'FAQPage')
                marked = [q['name'] for q in faq['mainEntity']]
                self.assertTrue(marked, 'no questions in the FAQ markup')
                for question in marked:
                    self.assertIn(question, rendered)

    def test_every_screenshot_ships_webp_with_a_png_fallback(self):
        for name in ('index.html', 'zh/index.html'):
            with self.subTest(page=name):
                html = (self.DOCS / name).read_text()
                pictures = re.findall(r'<picture>(.*?)</picture>', html, re.S)
                self.assertEqual(len(pictures), 6)  # 4 panels + agents + mobile
                for markup in pictures:
                    self.assertRegex(markup, r'<source type="image/webp" srcset="[^"]+\.webp')
                    # The PNG stays the src: a browser without WebP still shows
                    # the screenshot, and "open full-size" hands out a PNG.
                    self.assertRegex(markup, r'<img src="[^"]+\.png')
                    self.assertIn('alt="', markup)
                # Width and height stay on the img, so the box is reserved
                # before either format loads.
                self.assertEqual(len(re.findall(r'<img [^>]*width="\d+" height="\d+"', html)),
                                 html.count('<img '))

    def test_a_webp_exists_for_every_published_screenshot(self):
        shots = sorted((self.DOCS / 'screenshots/current').glob('*.png'))
        self.assertEqual(len(shots), 12)
        for png in shots:
            with self.subTest(shot=png.name):
                webp = png.with_suffix('.webp')
                self.assertTrue(webp.is_file(), f'{webp.name} missing: run scripts/site/encode_webp.py')
                self.assertLess(webp.stat().st_size, png.stat().st_size)

    def test_picture_is_not_left_inline(self):
        # An inline wrapper reports a zero-width box to everything measuring
        # the image's parent, including verify.mjs.
        self.assertRegex((self.DOCS / 'site.css').read_text(), r'picture\s*\{[^}]*display:\s*block')

    def test_the_404_page_serves_both_languages_and_stays_out_of_the_index(self):
        html = (self.DOCS / '404.html').read_text()
        self.assertIn('<meta name="robots" content="noindex">', html)
        # Both languages must be in the markup: the page is served for any
        # missing path, so there is no per-language URL to send anyone to.
        self.assertIn('data-nf="en"', html)
        self.assertIn('data-nf="zh"', html)
        # locale.js would redirect a preferred-language visitor to an index
        # page, turning a broken link into a silent bounce to the home page.
        self.assertNotIn('locale.js', html)
        # No canonical either: a 404 is not a page with a preferred URL.
        self.assertNotIn('rel="canonical"', html)

    def test_the_404_page_is_not_in_the_sitemap(self):
        self.assertNotIn('404', (self.DOCS / 'sitemap.xml').read_text())

    def test_the_sitemap_and_robots_are_generated_not_stale(self):
        from build import PAGE_PAIRS
        self.assertEqual((self.DOCS / 'sitemap.xml').read_text(), seo.sitemap(PAGE_PAIRS))
        self.assertEqual((self.DOCS / 'robots.txt').read_text(), seo.robots())


if __name__ == '__main__':
    unittest.main()
