#!/usr/bin/env python3
"""Render the mobile app's privacy policy page, one file per language.

The App Store and Google Play both require a public privacy-policy URL, so the
page has to live on the published site rather than in the repo. Its text is in
content/privacy.json; this module is layout only. It reuses the prose-page
styles (`doc-page`) but carries no download call to action or FAQ: a legal page
that ends in a sales pitch reads as one.

A section is a list of blocks, each exactly one of {"p": html}, {"h3": html} or
{"ul": [html, ...]}, so subheadings and lists need no markup in the JSON.
"""
from html import escape


def _block(block):
    if 'p' in block:
        return f'<p>{block["p"]}</p>'
    if 'h3' in block:
        return f'<h3>{block["h3"]}</h3>'
    if 'ul' in block:
        return '<ul>' + ''.join(f'<li>{item}</li>' for item in block['ul']) + '</ul>'
    raise ValueError(f'unknown privacy block: {sorted(block)}')


def render(lang, page, *, asset, home, other, github, social_head):
    """One page. `home`/`other` are relative links to the home page and the other language."""
    home_link = f'{home}{"zh/" if lang == "zh" else ""}'
    sections = ''.join(
        f'<section><h2>{section["h2"]}</h2>{"".join(_block(b) for b in section["blocks"])}</section>'
        for section in page['sections'])
    return f'''<!doctype html>
<html lang="{'zh-CN' if lang == 'zh' else 'en'}">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{page['title']}</title><meta name="description" content="{escape(page['description'], quote=True)}">
{social_head}
<link rel="icon" href="{home}icon.png">
<link rel="stylesheet" href="{asset('site.css')}">
</head>
<body data-locale="{lang}" class="doc-page">
<a class="skip" href="#main">{page['skip']}</a>
<header class="header"><a class="brand" href="{home_link}"><img src="{home}icon.png" width="32" height="32" alt="">Claw Fleet</a>
<nav aria-label="{'主导航' if lang == 'zh' else 'Main navigation'}"><a href="{home_link}">{page['backHome']}</a><a class="language" href="{other}" lang="{'en' if lang == 'zh' else 'zh-CN'}" hreflang="{'en' if lang == 'zh' else 'zh-CN'}">{'English' if lang == 'zh' else '中文'}</a></nav></header>
<main id="main" class="wrap">
<article class="doc doc-legal">
<p class="doc-eyebrow">{page['eyebrow']}</p>
<h1>{page['h1']}</h1>
<p class="doc-effective">{page['effective']}</p>
<p class="doc-lede">{page['lede']}</p>
{sections}
</article>
</main>
<footer class="wrap doc-footer"><p>{page['footer']}</p><p><a href="{github}">{'源码' if lang == 'zh' else 'Source'}</a> · <a href="{home_link}">{page['backHome']}</a></p></footer>
</body></html>'''
