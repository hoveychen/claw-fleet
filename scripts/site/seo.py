#!/usr/bin/env python3
"""The machine-readable half of every page: canonical, hreflang, social cards, JSON-LD.

One module because the landing page and the field report had already drifted --
benchmark.html carried no hreflang at all while index.html did, so the Chinese
field report was an orphan to every crawler. Each generator now asks for the
same block and passes only what actually differs per page.

Absolute URLs are written with site_origin.TOKEN and resolved per deployment;
see site_origin.py for why they cannot be baked in.
"""
from html import escape
import json
import re
import struct

import site_origin

TOKEN = site_origin.TOKEN
SITE_NAME = 'Claw Fleet'
OG_LOCALE = {'en': 'en_US', 'zh': 'zh_CN'}
LANG_TAG = {'en': 'en', 'zh': 'zh-CN'}
GITHUB = 'https://github.com/hoveychen/claw-fleet'
THEME_COLOR = '#ffffff'
# Search-console ownership proofs, as {meta name: content}. Empty until 老板
# hands over the codes; kept here rather than in the content files because it
# is the same proof in every language and has nothing to do with copy.
# The file-based alternative (a google*.html / baidu_verify_*.html at the site
# root) is allowed through by stage_pages.VERIFICATION_GLOBS, so either form
# works without touching the generator.
VERIFICATION = {}


def plain(markup):
    """Flatten copy that carries layout markup into a single line of text.

    Headlines in the content files contain `<br>` for their line breaks. Meta
    attributes are plain text, so leaving it in ships a literal "&lt;br&gt;" in
    the og:image:alt a crawler reads.
    """
    return ' '.join(re.sub(r'<[^>]+>', ' ', markup).split())


def png_size(path):
    """(width, height) straight out of the PNG IHDR, so og:image never guesses."""
    return struct.unpack('>II', path.read_bytes()[16:24])


def url(path=''):
    """Absolute (post-substitution) URL for a site-root-relative path."""
    if path.startswith('/'):
        raise ValueError('Site paths are relative to the site root, without a leading slash')
    return f'{TOKEN}/{path}'


def graph(nodes):
    """Wrap JSON-LD nodes into one script tag.

    One `@graph` rather than several scripts, so the nodes can reference each
    other by `@id` (the app's publisher is the same node the site declares).
    """
    payload = json.dumps({'@context': 'https://schema.org', '@graph': nodes},
                         ensure_ascii=False, separators=(',', ':'))
    # A literal "</" inside a script element would end it early.
    return '<script type="application/ld+json">' + payload.replace('</', '<\\/') + '</script>'


def publisher_node():
    return {
        '@type': 'Organization',
        '@id': url('#publisher'),
        'name': SITE_NAME,
        'url': url(),
        'logo': url('icon.png'),
        'sameAs': [GITHUB],
    }


def website_node(lang, description):
    return {
        '@type': 'WebSite',
        '@id': url('#website'),
        'name': SITE_NAME,
        'url': url(),
        'description': plain(description),
        'inLanguage': LANG_TAG[lang],
        'publisher': {'@id': url('#publisher')},
    }


def software_node(lang, description, screenshots, page_path):
    """The download page as a SoftwareApplication.

    Deliberately without aggregateRating or a review count: there is no rating
    to report, and an invented one is the kind of markup that gets a site's
    structured data ignored wholesale.
    """
    return {
        '@type': 'SoftwareApplication',
        '@id': url('#app'),
        'name': SITE_NAME,
        'url': url(page_path),
        'description': plain(description),
        'applicationCategory': 'DeveloperApplication',
        'operatingSystem': 'macOS, Windows, Linux, Android',
        # Resolved at publish time from the release being published; see
        # site_origin.VERSION_TOKEN for why it is not written at build time.
        'softwareVersion': site_origin.VERSION_TOKEN,
        'inLanguage': LANG_TAG[lang],
        'isAccessibleForFree': True,
        'license': f'{GITHUB}/blob/main/LICENSE',
        'downloadUrl': f'{GITHUB}/releases/latest',
        'softwareHelp': f'{GITHUB}/blob/main/README.md',
        'screenshot': [url(path) for path in screenshots],
        'offers': {'@type': 'Offer', 'price': '0', 'priceCurrency': 'USD'},
        'publisher': {'@id': url('#publisher')},
    }


def faq_node(faqs, page_path):
    """FAQPage built from the same pairs the page renders, so the two cannot drift."""
    return {
        '@type': 'FAQPage',
        '@id': url(page_path + '#faq'),
        'mainEntity': [{
            '@type': 'Question',
            'name': plain(question),
            'acceptedAnswer': {'@type': 'Answer', 'text': plain(answer)},
        } for question, answer in faqs],
    }


def breadcrumb_node(items, page_path):
    """items is [(name, site-relative path), ...] ending with the current page."""
    return {
        '@type': 'BreadcrumbList',
        '@id': url(page_path + '#breadcrumb'),
        'itemListElement': [{
            '@type': 'ListItem',
            'position': i,
            'name': plain(name),
            'item': url(path),
        } for i, (name, path) in enumerate(items, start=1)],
    }


def sitemap(page_pairs):
    """A sitemap for the whole site, each entry declaring its language alternates.

    `page_pairs` is [(en_path, zh_path), ...]. Both members of a pair appear as
    their own <url>, and each carries the full xhtml:link set including itself,
    which is what Google's hreflang-via-sitemap form requires.

    No <lastmod>, <changefreq> or <priority>: Google ignores the last two
    outright and distrusts a lastmod it catches being wrong. Stamping build time
    onto four pages that did not change would be exactly that.
    """
    urls = ''
    for en_path, zh_path in page_pairs:
        alternates = (f'<xhtml:link rel="alternate" hreflang="en" href="{url(en_path)}"/>'
                      f'<xhtml:link rel="alternate" hreflang="zh-CN" href="{url(zh_path)}"/>'
                      f'<xhtml:link rel="alternate" hreflang="x-default" href="{url(en_path)}"/>')
        for path in (en_path, zh_path):
            urls += f'\n<url><loc>{url(path)}</loc>{alternates}</url>'
    return ('<?xml version="1.0" encoding="UTF-8"?>\n'
            '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"'
            ' xmlns:xhtml="http://www.w3.org/1999/xhtml">'
            f'{urls}\n</urlset>\n')


def robots():
    """robots.txt for a deployment served from the root of its own origin.

    Note this only takes effect on the mirror: a crawler reads robots.txt from
    the origin root, and on GitHub Pages the site lives under /claw-fleet/, so
    hoveychen.github.io/robots.txt is not ours to write. Pages therefore relies
    on submitting the sitemap in Search Console instead.
    """
    return ('User-agent: *\n'
            'Allow: /\n'
            '\n'
            f'Sitemap: {url("sitemap.xml")}\n')


def head(*, lang, en_path, zh_path, title, description, image, image_size=None,
         image_alt='', page_type='website', verify_ownership=False):
    """Canonical + hreflang + OpenGraph + Twitter for one page, as an HTML block.

    `en_path` / `zh_path` are the canonical, site-root-relative paths of this
    page's two language versions ('' for the English home page, 'zh/' for the
    Chinese one). Directory form is the canonical form: index.html and ?lang=
    variants exist as links and must fold into it.

    `verify_ownership` emits the search-console proofs from VERIFICATION. Only
    the home pages ask for it: a proof is checked at the URL you submitted, and
    repeating it site-wide is noise in every other page's head.
    """
    self_path = en_path if lang == 'en' else zh_path
    other_lang = 'zh' if lang == 'en' else 'en'
    desc = escape(plain(description), quote=True)
    image_alt = plain(image_alt)
    tags = [f'<meta name="{name}" content="{escape(content, quote=True)}">'
            for name, content in VERIFICATION.items()] if verify_ownership else []
    tags += [
        f'<link rel="canonical" href="{url(self_path)}">',
        f'<link rel="alternate" hreflang="en" href="{url(en_path)}">',
        f'<link rel="alternate" hreflang="zh-CN" href="{url(zh_path)}">',
        f'<link rel="alternate" hreflang="x-default" href="{url(en_path)}">',
        f'<meta name="color-scheme" content="light">',
        f'<meta name="theme-color" content="{THEME_COLOR}">',
        f'<meta property="og:type" content="{page_type}">',
        f'<meta property="og:site_name" content="{SITE_NAME}">',
        f'<meta property="og:locale" content="{OG_LOCALE[lang]}">',
        f'<meta property="og:locale:alternate" content="{OG_LOCALE[other_lang]}">',
        f'<meta property="og:url" content="{url(self_path)}">',
        f'<meta property="og:title" content="{escape(title, quote=True)}">',
        f'<meta property="og:description" content="{desc}">',
        f'<meta property="og:image" content="{url(image)}">',
    ]
    if image_size:
        tags.append(f'<meta property="og:image:width" content="{image_size[0]}">')
        tags.append(f'<meta property="og:image:height" content="{image_size[1]}">')
    if image_alt:
        tags.append(f'<meta property="og:image:alt" content="{escape(image_alt, quote=True)}">')
    tags += [
        '<meta name="twitter:card" content="summary_large_image">',
        f'<meta name="twitter:title" content="{escape(title, quote=True)}">',
        f'<meta name="twitter:description" content="{desc}">',
        f'<meta name="twitter:image" content="{url(image)}">',
    ]
    if image_alt:
        tags.append(f'<meta name="twitter:image:alt" content="{escape(image_alt, quote=True)}">')
    return ''.join(tags)
