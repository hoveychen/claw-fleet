#!/usr/bin/env python3
"""Generate both static landing pages from shared markup and localized content."""
from pathlib import Path
from html import escape
import json

ROOT = Path(__file__).resolve().parents[2]
GITHUB = 'https://github.com/hoveychen/claw-fleet'
ASSETS = [('claw-fleet-macos.pkg','macOS'),('claw-fleet-windows-x64-setup.exe','Windows'),('fleet-linux-x64','x64'),('fleet-linux-arm64','ARM64')]


def build(lang, c):
    base = '../' if lang == 'zh' else './'
    other = '../index.html?lang=en' if lang == 'zh' else 'zh/index.html?lang=zh'
    def dl(name, label):
        url=f'{GITHUB}/releases/latest/download/{name}'
        return f'<div class="download-link"><a class="button" data-asset="{name}" href="{url}">{label}<span aria-hidden="true">↓</span></a><a class="fallback" href="{url}" hidden>{c["fallback"]}</a></div>'
    rows=''
    for i,(name,arch,desc) in enumerate(c['platforms']):
        links=dl(*ASSETS[i]) if i<2 else dl(*ASSETS[2])+dl(*ASSETS[3])
        rows+=f'<div class="download-row"><div class="platform"><img src="{base}icon-{["apple","windows","linux"][i]}.svg" width="28" height="28" alt=""><h3>{name}</h3></div><div class="architecture"><strong>{arch}</strong><span>{desc}</span></div><div class="download-actions">{links}</div></div>'
    features=''.join(f'<article><h3>{h}</h3><p>{p}</p></article>' for h,p in c['features'])
    more=''.join(f'<article><h3>{h}</h3><p>{p}</p></article>' for h,p in c['moreItems'])
    steps=''.join(f'<li><h3>{h}</h3><p>{p}</p></li>' for h,p in c['steps'])
    faq=''.join(f'<details><summary>{h}<span aria-hidden="true">+</span></summary><p>{p}</p></details>' for h,p in c['faqs'])
    shots=[f'work-{lang}.png',f'review-{lang}.png',f'results-{lang}.png']
    dimensions=[(1206,820),(1000,700),(1206,720)]
    panels=''
    for i,shot in enumerate(shots):
        w,h=dimensions[i]
        panels+=f'''<div id="panel-{i}" class="demo-panel">
<div class="product-stage stage-{i}"><div class="product-window"><img src="{base}screenshots/current/{shot}" width="{w}" height="{h}" {'fetchpriority="high"' if i==0 else 'loading="lazy"'} alt="{c['panelTitles'][i]}"></div></div>
<div class="panel-caption"><h3>{c['panelTitles'][i]}</h3><p>{c['panelCopy'][i]}</p></div></div>'''
    return f'''<!doctype html>
<html lang="{'zh-CN' if lang=='zh' else 'en'}">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{c['title']}</title><meta name="description" content="{escape(c['description'],quote=True)}">
<meta name="color-scheme" content="light"><meta property="og:title" content="{c['title']}"><meta property="og:description" content="{escape(c['description'],quote=True)}"><meta property="og:type" content="website"><meta property="og:image" content="https://hoveychen.github.io/claw-fleet/screenshots/current/work-{lang}.png"><meta name="twitter:card" content="summary_large_image">
<link rel="alternate" hreflang="en" href="{base}index.html"><link rel="alternate" hreflang="zh-CN" href="{base}zh/index.html"><link rel="alternate" hreflang="x-default" href="{base}index.html">
<script src="{base}locale.js"></script><link rel="icon" href="{base}icon.png"><link rel="stylesheet" href="{base}site.css"><script src="{base}site.js" defer></script>
</head>
<body data-locale="{lang}">
<a class="skip" href="#main">{c['skip']}</a>
<header class="header"><a class="brand" href="{base}{'zh/' if lang=='zh' else ''}"><img src="{base}icon.png" width="32" height="32" alt="">Claw Fleet</a><nav aria-label="{'主导航' if lang=='zh' else 'Main navigation'}"><a class="nav-explore" href="#explore">{c['nav'][0]}</a><a class="nav-mobile" href="#mobile">{c['nav'][1]}</a><a class="language" href="{other}" lang="{'en' if lang=='zh' else 'zh-CN'}" hreflang="{'en' if lang=='zh' else 'zh-CN'}">{c['language']}</a><a class="nav-download" href="#download">{c['nav'][2]}<span aria-hidden="true"> ↓</span></a></nav></header>
<main id="main">
<section class="hero wrap"><h1>{c['headline']}</h1><div class="hero-copy"><p class="intro">{c['intro']}</p><p>{c['lede']}</p><a class="button primary" href="#download">{c['cta']}<span aria-hidden="true">↓</span></a><a class="text-link" href="#explore">{c['secondary']} <span aria-hidden="true">↗</span></a><small>{c['meta']}</small></div></section>
<section class="showcase wrap" id="explore" aria-label="{c['nav'][0]}"><span id="demo"></span>
<div class="showcase-toolbar"><div class="tabs" aria-label="{c['sample']}">{''.join(f'<a id="tab-{i}" class="tab" href="#panel-{i}">{label}</a>' for i,label in enumerate(c['tabs']))}</div><span class="sample">{c['sample']}</span></div>
{panels}<p class="mobile-sample">{c['sample']}</p>
</section>
<section class="overview wrap"><div class="section-heading"><h2>{c['sectionHeading']}</h2><p>{c['sectionText']}</p></div><div class="feature-columns">{features}</div></section>
<section class="mobile-section wrap" id="mobile"><div class="mobile-art"><div class="phone"><img src="{base}screenshots/current/mobile-{lang}.png" width="430" height="932" loading="lazy" alt="{c['mobileAlt']}"></div><p>{c['mobileCaption']}</p></div><div class="mobile-copy"><h2>{c['mobileHeading']}</h2><p>{c['mobileCopy']}</p><ul>{''.join(f'<li>{p}</li>' for p in c['mobilePoints'])}</ul><a class="text-link" href="#getting-started">{c['mobileCta']} <span aria-hidden="true">↗</span></a></div></section>
<section class="work-depth wrap"><div class="section-heading"><h2>{c['moreHeading']}</h2><p>{c['moreCopy']}</p></div><div class="depth-list">{more}</div><div class="source-strip"><p>{c['sourceNames']}</p><span>{c['sourceBlurb']}</span></div></section>
<section class="download-section" id="download"><div class="wrap"><div class="section-heading"><h2>{c['downloadHeading']}</h2><p>{c['downloadCopy']}</p></div><p class="version-note">{c['versionNote']}</p><div class="download-source"><label for="download-source">{c['source']}</label><select id="download-source"><option value="github">{c['globalSource']}</option></select><p id="source-note" data-ready="{c['sourceReady']}" data-china="{c['chinaSource']}">{c['sourceNote']}</p></div><div class="downloads">{rows}</div><a class="text-link release-link" href="{GITHUB}/releases">{c['allReleases']} <span aria-hidden="true">↗</span></a><details class="linux-help"><summary>{c['linuxHelp']}<span aria-hidden="true">+</span></summary><pre><code>chmod +x fleet-linux-x64\n./fleet-linux-x64 webui</code></pre><p>{c['linuxAfter']}</p></details></div></section>
<section id="getting-started" class="getting-started wrap"><h2>{c['startHeading']}</h2><ol>{steps}</ol></section>
<section class="faq wrap"><h2>{c['faqHeading']}</h2><div>{faq}</div></section>
<section class="closing wrap"><h2>{c['closing']}</h2><a class="button primary" href="#download">{c['cta']}<span aria-hidden="true">↓</span></a></section>
</main>
<footer class="wrap"><div class="footer-top"><a class="brand" href="#main"><img src="{base}icon.png" width="28" height="28" alt="">Claw Fleet</a><div><a href="{GITHUB}">{c['sourceCode']}</a><a href="{GITHUB}/blob/main/README.md">{c['guide']}</a><a href="{GITHUB}/blob/main/LICENSE">{c['license']}</a><a class="language" href="{other}">{c['language']}</a></div></div><p>{c['footer']}</p></footer>
</body></html>'''


if __name__ == '__main__':
    for lang in ('en','zh'):
        content=json.loads((ROOT/f'scripts/site/content/{lang}.json').read_text())
        path=ROOT/'docs'/('zh/index.html' if lang=='zh' else 'index.html')
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_text(build(lang,content))
        print(path.relative_to(ROOT))
