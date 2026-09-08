#!/usr/bin/env python3
"""Render a prose page (one per agent tool, plus the tool list).

These pages answer a question the landing page does not: someone who already
runs the CLI and is searching for "claude code gui" wants to know what sits
around it. The landing page sells the positioning; these pages answer the
query.

Each page's text lives in content/{en,zh}.json under `pages`, so this module is
layout only -- and each page's body has to be substantially its own. Four pages
that differ by a product name are doorway pages in Google's own spam policy,
and that is a site-wide penalty, not a per-page one. The material that makes
them genuinely different is real: the three tools expose different model
catalogues, different reasoning-effort ladders and different sign-in paths.
"""
from html import escape

import seo


def render(lang, slug, page, *, asset, dims, home, other, github, social_head, structured):
    """One page. `page` is the content block; `home`/`other` are relative links."""
    sections = ''
    for section in page['sections']:
        body = ''.join(f'<p>{para}</p>' for para in section['body'])
        shot = ''
        if section.get('shot'):
            name = section['shot']
            # Dimensions come from the file, never from the content JSON: a
            # hand-typed size that drifts from a re-captured screenshot moves
            # the layout after paint.
            width, height = dims(name)
            shot = (f'<figure class="doc-shot">'
                    f'<picture><source type="image/webp" srcset="{asset(name.replace(".png", ".webp"))}">'
                    f'<img src="{asset(name)}" width="{width}" height="{height}"'
                    f' loading="lazy" alt="{escape(section["shotAlt"], quote=True)}"></picture>'
                    f'<figcaption>{section["shotCaption"]}</figcaption></figure>')
        sections += f'<section><h2>{section["h2"]}</h2>{body}{shot}</section>'
    faq = ''
    if page.get('faqs'):
        faq = ('<section class="doc-faq"><h2>' + page['faqHeading'] + '</h2>'
               + ''.join(f'<details><summary>{q}<span aria-hidden="true">+</span></summary><p>{a}</p></details>'
                         for q, a in page['faqs'])
               + '</section>')
    return f'''<!doctype html>
<html lang="{'zh-CN' if lang == 'zh' else 'en'}">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{page['title']}</title><meta name="description" content="{escape(page['description'], quote=True)}">
{social_head}
{structured}
<link rel="icon" href="{home}icon.png">
<link rel="stylesheet" href="{asset('site.css')}">
</head>
<body data-locale="{lang}" class="doc-page">
<a class="skip" href="#main">{page['skip']}</a>
<header class="header"><a class="brand" href="{home}{'zh/' if lang == 'zh' else ''}"><img src="{home}icon.png" width="32" height="32" alt="">Claw Fleet</a>
<nav aria-label="{'主导航' if lang == 'zh' else 'Main navigation'}"><a href="{home}{'zh/' if lang == 'zh' else ''}">{page['backHome']}</a><a href="{home}{'zh/' if lang == 'zh' else ''}#download">{page['navDownload']}</a><a class="language" href="{other}" lang="{'en' if lang == 'zh' else 'zh-CN'}" hreflang="{'en' if lang == 'zh' else 'zh-CN'}">{'English' if lang == 'zh' else '中文'}</a></nav></header>
<main id="main" class="wrap">
<article class="doc">
<p class="doc-eyebrow">{page['eyebrow']}</p>
<h1>{page['h1']}</h1>
<p class="doc-lede">{page['lede']}</p>
{sections}
{faq}
<section class="doc-close"><h2>{page['closingHeading']}</h2><p>{page['closing']}</p>
<a class="button primary" href="{home}{'zh/' if lang == 'zh' else ''}#download">{page['cta']}<span aria-hidden="true">↓</span></a></section>
</article>
</main>
<footer class="wrap doc-footer"><p>{page['footer']}</p><p><a href="{github}">{'源码' if lang == 'zh' else 'Source'}</a> · <a href="{home}{'zh/' if lang == 'zh' else ''}">{page['backHome']}</a></p></footer>
</body></html>'''
