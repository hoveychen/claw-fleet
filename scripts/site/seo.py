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
import re
import struct

import site_origin

TOKEN = site_origin.TOKEN
SITE_NAME = 'Claw Fleet'
OG_LOCALE = {'en': 'en_US', 'zh': 'zh_CN'}
THEME_COLOR = '#ffffff'


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


def head(*, lang, en_path, zh_path, title, description, image, image_size=None,
         image_alt='', page_type='website'):
    """Canonical + hreflang + OpenGraph + Twitter for one page, as an HTML block.

    `en_path` / `zh_path` are the canonical, site-root-relative paths of this
    page's two language versions ('' for the English home page, 'zh/' for the
    Chinese one). Directory form is the canonical form: index.html and ?lang=
    variants exist as links and must fold into it.
    """
    self_path = en_path if lang == 'en' else zh_path
    other_lang = 'zh' if lang == 'en' else 'en'
    desc = escape(plain(description), quote=True)
    image_alt = plain(image_alt)
    tags = [
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
