#!/usr/bin/env python3
"""Generate both static landing pages from shared markup and localized content."""
from pathlib import Path
from html import escape
import json
import hashlib

import seo

ROOT = Path(__file__).resolve().parents[2]
GITHUB = 'https://github.com/hoveychen/claw-fleet'
# (english path, chinese path) for every real page, in sitemap order. The pair
# is what ties the two language versions together in hreflang, so a new page
# joins the sitemap and gets its alternates from one edit.
PAGE_PAIRS = [('', 'zh/'), ('benchmark.html', 'zh/benchmark.html')]
# Prose pages generated from content['pages'], one entry per slug. Listed here
# so a new page joins PAGE_PAIRS (and therefore the sitemap, the hreflang set
# and the lastmod map) by adding its content block and one line.
CONTENT_SLUGS = ['claude-code-gui', 'codex-gui']
PAGE_PAIRS += [(f'{slug}.html', f'zh/{slug}.html') for slug in CONTENT_SLUGS]
# One entry per download row, positionally matched to content['platforms'].
# Each row names its icon and the buttons it carries, so a row with two builds
# (Linux) or a new platform (Android) is a data change, not an index trick.
ROWS = [
    ('apple', [('claw-fleet-macos.pkg', 'macOS')]),
    ('windows', [('claw-fleet-windows-x64-setup.exe', 'Windows')]),
    ('linux', [('fleet-linux-x64', 'x64'), ('fleet-linux-arm64', 'ARM64')]),
    ('android', [('claw-fleet-android.apk', 'APK')]),
]


def build(lang, c):
    base = '../' if lang == 'zh' else './'
    def asset(name):
        digest = hashlib.sha256((ROOT / 'docs' / name).read_bytes()).hexdigest()[:12]
        return f'{base}{name}?v={digest}'
    other = '../index.html?lang=en' if lang == 'zh' else 'zh/index.html?lang=zh'
    def dl(name, label):
        url=f'{GITHUB}/releases/latest/download/{name}'
        return f'<div class="download-link"><a class="button" data-asset="{name}" href="{url}">{label}<span aria-hidden="true">↓</span></a><a class="fallback" href="{url}" hidden>{c["fallback"]}</a></div>'
    rows=''
    if len(c['platforms']) != len(ROWS):
        raise ValueError(f'platforms/ROWS mismatch: {len(c["platforms"])} vs {len(ROWS)}')
    for (icon,buttons),(name,arch,desc) in zip(ROWS,c['platforms']):
        links=''.join(dl(*button) for button in buttons)
        rows+=f'<div class="download-row"><div class="platform"><img src="{base}icon-{icon}.svg" width="28" height="28" alt=""><h3>{name}</h3></div><div class="architecture"><strong>{arch}</strong><span>{desc}</span></div><div class="download-actions">{links}</div></div>'
    features=''.join(f'<article><h3>{h}</h3><p>{p}</p></article>' for h,p in c['features'])
    relay=('<span class="relay-arrow" aria-hidden="true">→</span>').join(
        f'<div class="relay-node"><strong>{escape(name)}</strong><span>{escape(role)}</span></div>'
        for name,role in c['relayNodes'])
    harness_points=''.join(f'<article><h3>{h}</h3><p>{p}</p></article>' for h,p in c['harnessPoints'])
    more=''.join(f'<article><h3>{h}</h3><p>{p}</p></article>' for h,p in c['moreItems'])
    steps=''.join(f'<li><h3>{h}</h3><p>{p}</p></li>' for h,p in c['steps'])
    faq=''.join(f'<details><summary>{h}<span aria-hidden="true">+</span></summary><p>{p}</p></details>' for h,p in c['faqs'])
    catalogue = json.loads((ROOT / 'scripts/site/content/capabilities.json').read_text())[lang]
    capability_count = sum(len(group['items']) for group in catalogue)
    capability_rows = ''.join(
        '<details class="capability-group"><summary><h3>' + escape(group['title']) + '</h3><span>' + str(len(group['items'])) + (' 项能力' if lang == 'zh' else ' capabilities') + '</span></summary><ul>'
        + ''.join('<li><h4>' + escape(title) + '</h4><p>' + escape(copy) + '</p></li>' for title, copy in group['items'])
        + '</ul></details>' for group in catalogue
    )
    catalogue_title = '工作台的每一面。' if lang == 'zh' else 'More of the workspace.'
    catalogue_copy = f'{capability_count} 项能力，按使用场景整理。需要时展开，不必一次学完。' if lang == 'zh' else f'{capability_count} capabilities, grouped by how you use them. Open a section when you need it.'
    catalogue_toggle = '展开全部' if lang == 'zh' else 'Expand all'
    catalogue_collapse = '收起全部' if lang == 'zh' else 'Collapse all'
    capabilities = f'<section class="capabilities wrap" id="capabilities"><div class="capabilities-heading"><div><h2>{catalogue_title}</h2><p>{catalogue_copy}</p></div><button class="catalogue-toggle" data-expand="{catalogue_toggle}" data-collapse="{catalogue_collapse}" aria-expanded="false">{catalogue_toggle}</button></div><div class="capability-list">{capability_rows}</div></section>'
    shots=[f'work-{lang}.png',f'review-{lang}.png',f'relay-{lang}.png',f'results-{lang}.png']
    def dimensions(name):
        return seo.png_size(ROOT / 'docs/screenshots/current' / name)
    def picture(name, w, h, alt, *, eager=False):
        """A screenshot as WebP with the PNG kept as the fallback <img>.

        WebP costs 71% fewer bytes on these flat UI shots (2.6 MB -> 758 KB
        across the set), which matters most to the visitors coming through the
        China mirror. The PNG stays the `src` so a browser that cannot decode
        WebP still shows the screenshot, and it stays what "open full-size"
        hands out -- a file anyone can save and paste anywhere.
        """
        loading = 'fetchpriority="high"' if eager else 'loading="lazy"'
        return (f'<picture><source type="image/webp" srcset="{asset(name.replace(".png", ".webp"))}">'
                f'<img src="{asset(name)}" width="{w}" height="{h}" {loading} alt="{alt}"></picture>')
    social_shot = f'screenshots/current/work-{lang}.png'
    page_path = 'zh/' if lang == 'zh' else ''
    structured = seo.graph([
        seo.publisher_node(),
        seo.website_node(lang, c['description']),
        seo.software_node(lang, c['description'],
                          [f'screenshots/current/{name}' for name in shots], page_path),
        seo.faq_node(c['faqs'], page_path),
    ])
    social_head = seo.head(
        lang=lang, en_path='', zh_path='zh/',
        title=c['title'], description=c['description'],
        image=social_shot, image_size=dimensions(f'work-{lang}.png'),
        image_alt=c['panelTitles'][0], verify_ownership=True)
    mobile_w, mobile_h = dimensions(f'mobile-{lang}.png')
    agents_w, agents_h = dimensions(f'agents-{lang}.png')
    agents_src = asset(f'screenshots/current/agents-{lang}.png')
    harness_shot = (f'<div class="product-stage harness-stage"><div class="product-window">'
        f'<a class="screenshot-open" href="{agents_src}" target="_blank" aria-label="' + ('查看完整截图' if lang=='zh' else 'Open full-size screenshot') + '">'
        + picture(f'screenshots/current/agents-{lang}.png', agents_w, agents_h, c['agentsAlt']) + '</a>'
        f'</div></div><p class="harness-sample">{c["sample"]}</p>')
    panels=''
    for i,shot in enumerate(shots):
        w,h=dimensions(shot)
        panels+=f'''<div id="panel-{i}" class="demo-panel">
<div class="product-stage stage-{i}"><div class="product-window"><a class="screenshot-open" href="{asset("screenshots/current/" + shot)}" target="_blank" aria-label="{'查看完整截图' if lang=='zh' else 'Open full-size screenshot'}">{picture("screenshots/current/" + shot, w, h, c['panelTitles'][i], eager=i==0)}</a></div></div>
<div class="panel-caption"><h3>{c['panelTitles'][i]}</h3><p>{c['panelCopy'][i]}</p></div></div>'''
    return f'''<!doctype html>
<html lang="{'zh-CN' if lang=='zh' else 'en'}">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{c['title']}</title><meta name="description" content="{escape(c['description'],quote=True)}">
{social_head}
{structured}
<script src="{asset('locale.js')}"></script><link rel="icon" href="{base}icon.png"><link rel="stylesheet" href="{asset('site.css')}"><script src="{asset('site.js')}" defer></script>
</head>
<body data-locale="{lang}">
<a class="skip" href="#main">{c['skip']}</a>
<header class="header"><a class="brand" href="{base}{'zh/' if lang=='zh' else ''}"><img src="{base}icon.png" width="32" height="32" alt="">Claw Fleet</a><nav aria-label="{'主导航' if lang=='zh' else 'Main navigation'}"><a class="nav-explore" href="#explore">{c['nav'][0]}</a><a class="nav-harness" href="#harness">{c['navHarness']}</a><a class="nav-capabilities" href="#capabilities">{'全部功能' if lang=='zh' else 'Features'}</a><a class="nav-benchmark" href="benchmark.html">{c['navBenchmark']}</a><a class="nav-mobile" href="#mobile">{c['nav'][1]}</a><a class="language" href="{other}" lang="{'en' if lang=='zh' else 'zh-CN'}" hreflang="{'en' if lang=='zh' else 'zh-CN'}">{c['language']}</a><a class="nav-download" href="#download">{c['nav'][2]}<span aria-hidden="true"> ↓</span></a></nav></header>
<main id="main">
<section class="hero wrap"><h1>{c['headline']}</h1><div class="hero-copy"><p class="intro">{c['intro']}</p><p>{c['lede']}</p><a class="button primary" href="#download">{c['cta']}<span aria-hidden="true">↓</span></a><a class="text-link" href="#explore">{c['secondary']} <span aria-hidden="true">↗</span></a><small>{c['meta']}</small></div></section>
<section class="showcase wrap" id="explore" aria-label="{c['nav'][0]}"><span id="demo"></span>
<div class="showcase-toolbar"><div class="tabs" aria-label="{c['sample']}">{''.join(f'<a id="tab-{i}" class="tab" href="#panel-{i}">{label}</a>' for i,label in enumerate(c['tabs']))}</div><span class="sample">{c['sample']}</span></div>
{panels}<p class="mobile-sample">{c['sample']}</p>
</section>
<section class="overview wrap"><div class="section-heading"><h2>{c['sectionHeading']}</h2><p>{c['sectionText']}</p></div><div class="feature-columns">{features}</div></section>
<section class="harness wrap" id="harness"><div class="section-heading"><h2>{c['harnessHeading']}</h2><p>{c['harnessCopy']}</p></div><div class="relay"><div class="relay-lane">{relay}</div><p class="relay-note">{c['relayNote']}</p></div>{harness_shot}<div class="feature-columns">{harness_points}</div></section>
<section class="mobile-section wrap" id="mobile"><div class="mobile-art"><div class="phone">{picture(f'screenshots/current/mobile-{lang}.png', mobile_w, mobile_h, c['mobileAlt'])}</div><p>{c['mobileCaption']}</p></div><div class="mobile-copy"><h2>{c['mobileHeading']}</h2><p>{c['mobileCopy']}</p><ul>{''.join(f'<li>{p}</li>' for p in c['mobilePoints'])}</ul><a class="text-link" href="#getting-started">{c['mobileCta']} <span aria-hidden="true">↗</span></a></div></section>
<section class="work-depth wrap"><div class="section-heading"><h2>{c['moreHeading']}</h2><p>{c['moreCopy']}</p></div><div class="depth-list">{more}</div><div class="source-strip"><p>{c['sourceNames']}</p><span>{c['sourceBlurb']}</span></div></section>
{capabilities}
<section class="download-section" id="download"><div class="wrap"><div class="section-heading"><h2>{c['downloadHeading']}</h2><p>{c['downloadCopy']}</p></div><p class="version-note">{c['versionNote']}</p><div class="download-source"><label for="download-source">{c['source']}</label><select id="download-source"><option value="github">{c['globalSource']}</option></select><p id="source-note" data-version="{c['sourceVersion']}" data-ready="{c['sourceReady']}" data-china="{c['chinaSource']}">{c['sourceNote']}</p></div><div class="downloads">{rows}</div><a class="text-link release-link" href="{GITHUB}/releases">{c['allReleases']} <span aria-hidden="true">↗</span></a><details class="linux-help"><summary>{c['linuxHelp']}<span aria-hidden="true">+</span></summary><pre><code>chmod +x fleet-linux-x64\n./fleet-linux-x64 webui</code></pre><p>{c['linuxAfter']}</p></details><details class="android-help"><summary>{c['androidHelp']}<span aria-hidden="true">+</span></summary><p>{c['androidAfter']}</p></details></div></section>
<section id="getting-started" class="getting-started wrap"><h2>{c['startHeading']}</h2><ol>{steps}</ol></section>
<section class="faq wrap"><h2>{c['faqHeading']}</h2><div>{faq}</div></section>
<section class="closing wrap"><h2>{c['closing']}</h2><a class="button primary" href="#download">{c['cta']}<span aria-hidden="true">↓</span></a></section>
</main>
<footer class="wrap"><div class="footer-top"><a class="brand" href="#main"><img src="{base}icon.png" width="28" height="28" alt="">Claw Fleet</a><div><a href="{GITHUB}">{c['sourceCode']}</a><a href="{GITHUB}/blob/main/README.md">{c['guide']}</a><a href="{GITHUB}/blob/main/LICENSE">{c['license']}</a><a class="language" href="{other}">{c['language']}</a></div></div><p>{c['footer']}</p></footer>
</body></html>'''


def build_404(content, asset):
    """One page for both languages, served for any missing path.

    Deliberately does NOT load locale.js: that script redirects to the other
    language's index.html when the browser prefers it, which on a 404 would
    bounce the visitor to the home page and hide the broken link entirely.
    Both languages are in the markup and a tiny inline script picks one, so a
    missing page stays a missing page.

    noindex because the mirror serves this file through nginx's error_page and
    a misconfiguration there could hand it out with a 200.
    """
    def block(lang, c):
        base = './'
        return (f'<div class="nf" lang="{"zh-CN" if lang == "zh" else "en"}" data-nf="{lang}">'
                f'<h1>{c["heading"]}</h1><p>{c["copy"]}</p>'
                f'<a class="button primary" href="{base}{"zh/" if lang == "zh" else ""}">{c["home"]}</a>'
                f'<a class="text-link" href="{base}{"zh/" if lang == "zh" else ""}#download">{c["download"]}</a>'
                f'<a class="text-link" href="{base}{"zh/benchmark.html" if lang == "zh" else "benchmark.html"}">{c["report"]}</a>'
                '</div>')
    en, zh = content['en']['notFound'], content['zh']['notFound']
    return f'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{en['title']} / {zh['title']}</title>
<meta name="robots" content="noindex">
<meta name="color-scheme" content="light"><meta name="theme-color" content="{seo.THEME_COLOR}">
<link rel="icon" href="./icon.png"><link rel="stylesheet" href="{asset('site.css')}">
<script>
// No redirect, no framework: read the language the visitor already chose on
// this site, fall back to the browser, and reveal that half of the page.
(() => {{
  let saved; try {{ saved = localStorage.getItem('claw-fleet-site-language'); }} catch {{}}
  const preferred = navigator.languages?.[0] || navigator.language || 'en';
  const lang = saved === 'zh' || saved === 'en' ? saved : (/^zh(?:-|$)/i.test(preferred) ? 'zh' : 'en');
  document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en';
  document.addEventListener('DOMContentLoaded', () => document.body.dataset.locale = lang);
}})();
</script>
</head>
<body data-locale="en" class="nf-page">
<header class="header"><a class="brand" href="./"><img src="./icon.png" width="32" height="32" alt="">Claw Fleet</a></header>
<main id="main">{block('en', en)}{block('zh', zh)}</main>
</body></html>'''


if __name__ == '__main__':
    import benchmark
    import content_page
    bm_copy=json.loads((ROOT/'scripts/site/content/benchmark.json').read_text())
    bm_data=json.loads((ROOT/'scripts/site/content/benchmark-data.json').read_text())
    for lang in ('en','zh'):
        content=json.loads((ROOT/f'scripts/site/content/{lang}.json').read_text())
        path=ROOT/'docs'/('zh/index.html' if lang=='zh' else 'index.html')
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_text(build(lang,content))
        print(path.relative_to(ROOT))
        base='../' if lang=='zh' else './'
        def asset(name,base=base):
            digest=hashlib.sha256((ROOT/'docs'/name).read_bytes()).hexdigest()[:12]
            return f'{base}{name}?v={digest}'
        def dims(name, _root=ROOT):
            return seo.png_size(_root / 'docs' / name)
        for slug in CONTENT_SLUGS:
            page = content['pages'][slug]
            en_path, zh_path = f'{slug}.html', f'zh/{slug}.html'
            page_path = zh_path if lang == 'zh' else en_path
            shot = next((s['shot'] for s in page['sections'] if s.get('shot')), None)
            head = seo.head(lang=lang, en_path=en_path, zh_path=zh_path,
                            title=page['title'], description=page['description'],
                            image=shot or f'screenshots/current/work-{lang}.png',
                            image_size=dims(shot) if shot else None,
                            image_alt=page['h1'], page_type='article')
            structured = seo.graph([
                seo.publisher_node(),
                seo.breadcrumb_node([(seo.SITE_NAME, 'zh/' if lang == 'zh' else ''),
                                     (page['title'], page_path)], page_path),
                seo.faq_node(page['faqs'], page_path),
            ])
            out = ROOT / 'docs' / page_path
            out.write_text(content_page.render(
                lang, slug, page, asset=asset, dims=dims, home=base,
                other=('../' + en_path) if lang == 'zh' else zh_path,
                github=GITHUB, social_head=head, structured=structured))
            print(out.relative_to(ROOT))
        bm=ROOT/'docs'/('zh/benchmark.html' if lang=='zh' else 'benchmark.html')
        bm.write_text(benchmark.build(lang,bm_copy[lang],bm_data,asset,base,
            '../benchmark.html' if lang=='zh' else 'zh/benchmark.html',GITHUB))
        print(bm.relative_to(ROOT))
    # Nothing links to these two, so they are generated here rather than
    # discovered; distribute.site_files() lists them explicitly for the mirror.
    all_content = {lang: json.loads((ROOT / f'scripts/site/content/{lang}.json').read_text())
                   for lang in ('en', 'zh')}
    def root_asset(name):
        digest = hashlib.sha256((ROOT / 'docs' / name).read_bytes()).hexdigest()[:12]
        return f'./{name}?v={digest}'
    for name, body in (('404.html', build_404(all_content, root_asset)),
                       ('sitemap.xml', seo.sitemap(PAGE_PAIRS)), ('robots.txt', seo.robots())):
        (ROOT / 'docs' / name).write_text(body)
        print(f'docs/{name}')
