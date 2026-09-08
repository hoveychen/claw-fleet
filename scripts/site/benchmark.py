#!/usr/bin/env python3
"""Generate the /benchmark field-report page from measured data + localized copy.

Every figure on the page comes from scripts/site/content/benchmark-data.json,
which is produced by re-reading the local transcripts — the page never carries a
hand-typed number, so refreshing the report is a data change, not an edit pass.
"""
from html import escape

# Chart geometry for the daily curve. One place, so the axis, the grid and the
# hover targets cannot drift apart.
W, H = 960, 250
PAD_L, PAD_R, PAD_T, PAD_B = 46, 14, 16, 30
Y_MAX = 30000
Y_TICKS = (0, 10000, 20000, 30000)


def _x(i, n):
    return PAD_L + (W - PAD_L - PAD_R) * (i / (n - 1))


def _y(v):
    return PAD_T + (H - PAD_T - PAD_B) * (1 - v / Y_MAX)


def _daily_chart(daily, c, lang):
    n = len(daily)
    pts = [(_x(i, n), _y(calls)) for i, (_, calls, _s) in enumerate(daily)]
    line = ' '.join(f'{x:.1f},{y:.1f}' for x, y in pts)
    area = f'{pts[0][0]:.1f},{_y(0):.1f} ' + line + f' {pts[-1][0]:.1f},{_y(0):.1f}'
    grid = ''.join(
        f'<line x1="{PAD_L}" x2="{W - PAD_R}" y1="{_y(t):.1f}" y2="{_y(t):.1f}" class="bm-grid"></line>'
        f'<text x="{PAD_L - 10}" y="{_y(t) + 4:.1f}" class="bm-tick bm-tick-y">{t // 1000}k</text>'
        for t in Y_TICKS)
    # x labels every 7th day, plus hover targets over every day
    xlabels = ''.join(
        f'<text x="{_x(i, n):.1f}" y="{H - 8}" class="bm-tick">{escape(day[5:])}</text>'
        for i, (day, _c, _s) in enumerate(daily) if i % 7 == 0)
    band = (W - PAD_L - PAD_R) / (n - 1)
    hits = ''
    for i, (day, calls, sess) in enumerate(daily):
        x, y = pts[i]
        label = (f'{day} · {calls:,} 次调用 · {sess} 个会话' if lang == 'zh'
                 else f'{day} · {calls:,} calls · {sess} sessions live')
        hits += (f'<g class="bm-hit"><rect x="{x - band / 2:.1f}" y="{PAD_T}" width="{band:.1f}" '
                 f'height="{H - PAD_T - PAD_B}"><title>{escape(label)}</title></rect>'
                 f'<circle cx="{x:.1f}" cy="{y:.1f}" r="4.5"></circle></g>')
    peak_i = max(range(n), key=lambda i: daily[i][1])
    px, py = pts[peak_i]
    peak_val = daily[peak_i][1]
    anchor = 'end' if px > W * 0.72 else 'start'
    dx = -10 if anchor == 'end' else 10
    peak = (f'<circle cx="{px:.1f}" cy="{py:.1f}" r="4.5" class="bm-dot"></circle>'
            f'<text x="{px + dx:.1f}" y="{py - 12:.1f}" text-anchor="{anchor}" class="bm-peak">{peak_val:,}</text>')
    rows = ''.join(
        f'<tr><td>{day}</td><td class="num">{calls:,}</td><td class="num">{sess}</td></tr>'
        for day, calls, sess in daily)
    return f'''<figure class="bm-chart">
<p class="bm-chart-title">{c['dailyTitle']}</p><p class="bm-chart-sub">{c['dailySub']}</p>
<svg viewBox="0 0 {W} {H}" class="bm-svg" role="img" aria-label="{escape(c['dailyTitle'], quote=True)}">
{grid}<polygon points="{area}" class="bm-area"></polygon>
<polyline points="{line}" class="bm-line"></polyline>{peak}
<line x1="{PAD_L}" x2="{W - PAD_R}" y1="{_y(0):.1f}" y2="{_y(0):.1f}" class="bm-axis"></line>
{xlabels}{hits}</svg>
<figcaption>{c['dailyCaption']}</figcaption>
<details class="bm-data"><summary>{c['tableToggle']}</summary>
<table><thead><tr>{''.join(f'<th>{h}</th>' for h in c['dailyTable'])}</tr></thead><tbody>{rows}</tbody></table>
</details></figure>'''


def _conc_chart(d, c):
    ge = d['concurrency']['ge']
    bars = ''.join(
        f'<div class="bm-bar"><p class="k">{c["concKey"].replace("{n}", str(n))}</p>'
        f'<div class="track"><div class="fill" style="width:{pct}%" tabindex="0" '
        f'title="{c["concKey"].replace("{n}", str(n))} · {pct}% · {hours} h"></div></div>'
        f'<p class="v">{pct}%</p></div>' for n, pct, hours in ge)
    rows = ''.join(f'<tr><td>{n}+</td><td class="num">{pct}%</td><td class="num">{hours} h</td></tr>'
                   for n, pct, hours in ge)
    rows += (f'<tr><td>peak</td><td class="num">{d["concurrency"]["peak"]}</td><td class="num">—</td></tr>')
    return f'''<figure class="bm-chart">
<p class="bm-chart-title">{c['concTitle']}</p><p class="bm-chart-sub">{c['concSub']}</p>
<div class="bm-bars">{bars}</div>
<div class="bm-haxis"><span></span><div class="ticks"><span>0</span><span>25%</span><span>50%</span><span>75%</span><span>100%</span></div><span></span></div>
<figcaption>{c['concCaption']}</figcaption>
<details class="bm-data"><summary>{c['tableToggle']}</summary>
<table><thead><tr>{''.join(f'<th>{h}</th>' for h in c['concTable'])}</tr></thead><tbody>{rows}</tbody></table>
</details></figure>'''


def _units_chart(d, c):
    # One square per call bought by a single human input; the first is the human's.
    squares = '<i class="human" title="1"></i>' + ''.join(
        '<i></i>' for _ in range(round(d['leverage']) - 1))
    note = ''.join(f'<span>{n}</span>' for n in c['unitsNote'])
    rows = (f'<tr><td>{c["unitsNote"][0].replace("<b>", "").replace("</b>", "")}</td>'
            f'<td class="num">{d["apiCalls"]:,}</td><td class="num">{d["leverage"]}×</td></tr>'
            f'<tr><td>{c["unitsNote"][1].replace("<b>", "").replace("</b>", "")}</td>'
            f'<td class="num">{d["humanTurns"]:,}</td><td class="num">1×</td></tr>'
            f'<tr><td>{c["unitsNote"][2].replace("<b>", "").replace("</b>", "")}</td>'
            f'<td class="num">{d["cards"]:,}</td>'
            f'<td class="num">{d["cards"] / d["humanTurns"]:.2f}×</td></tr>')
    return f'''<figure class="bm-chart">
<p class="bm-chart-title">{c['unitsTitle']}</p><p class="bm-chart-sub">{c['unitsSub']}</p>
<div class="bm-legend"><span><i class="human"></i>{c['unitsLegend'][0]}</span><span><i></i>{c['unitsLegend'][1]}</span></div>
<div class="bm-units" role="img" aria-label="{escape(c['unitsSub'], quote=True)}">{squares}</div>
<div class="bm-units-note">{note}</div>
<figcaption>{c['unitsCaption']}</figcaption>
<details class="bm-data"><summary>{c['tableToggle']}</summary>
<table><thead><tr>{''.join(f'<th>{h}</th>' for h in c['unitsTable'])}</tr></thead><tbody>{rows}</tbody></table>
</details></figure>'''


def build(lang, c, d, asset, home, other, github):
    tiles = ''.join(
        f'<div class="bm-tile"><p class="label">{label}</p><p class="value">{value}</p>'
        f'<p class="note">{note}</p></div>' for label, value, note in c['tiles'])
    caps = ''.join(
        f'<div class="bm-cap"><p class="m">{m}</p><h3>{title}</h3><p>{copy}</p>'
        f'<p class="base">{base}</p></div>' for m, title, copy, base in c['caps'])
    steps = ''.join(f'<li>{s}</li>' for s in c['methodSteps'])
    caveats = ''.join(f'<p>{s}</p>' for s in c['caveats'])
    faqs = ''.join(f'<details><summary>{q}<span aria-hidden="true">+</span></summary><p>{a}</p></details>'
                   for q, a in c['faqs'])
    return f'''<!doctype html>
<html lang="{'zh-CN' if lang == 'zh' else 'en'}">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{c['title']}</title><meta name="description" content="{escape(c['description'], quote=True)}">
<meta name="color-scheme" content="light">
<meta property="og:title" content="{c['title']}"><meta property="og:description" content="{escape(c['description'], quote=True)}"><meta property="og:type" content="article">
<link rel="icon" href="{home}icon.png">
<link rel="stylesheet" href="{asset('site.css')}"><link rel="stylesheet" href="{asset('benchmark.css')}">
</head>
<body data-locale="{lang}" class="bm-page">
<a class="skip" href="#main">{c['skip']}</a>
<header class="header"><a class="brand" href="{home}{'zh/' if lang == 'zh' else ''}"><img src="{home}icon.png" width="32" height="32" alt="">Claw Fleet</a>
<nav aria-label="{'主导航' if lang == 'zh' else 'Main navigation'}"><a href="{home}{'zh/' if lang == 'zh' else ''}">{c['backHome']}</a><a class="language" href="{other}" lang="{'en' if lang == 'zh' else 'zh-CN'}" hreflang="{'en' if lang == 'zh' else 'zh-CN'}">{'English' if lang == 'zh' else '中文'}</a></nav></header>
<main id="main">
<section class="bm-hero wrap">
<p class="bm-eyebrow">{c['eyebrow']} · {d['window']['from']} → {d['window']['to']}</p>
<h1>{c['headline']}</h1>
<p class="bm-lede">{c['lede']}</p>
<div class="bm-figure"><p class="bm-number">{d['leverage']}<small>×</small></p><p class="bm-figure-caption">{c['heroCaption']}</p></div>
<div class="bm-tiles">{tiles}</div>
</section>
<hr class="bm-rule">
<section class="wrap bm-section" id="throughput">
<div class="bm-head"><h2>{c['throughputHeading']}</h2><p>{c['throughputCopy']}</p></div>
{_conc_chart(d, c)}
<div class="bm-gap">{_daily_chart(d['daily'], c, lang)}</div>
<div class="bm-gap">{_units_chart(d, c)}</div>
</section>
<hr class="bm-rule">
<section class="wrap bm-section" id="capability">
<div class="bm-head"><h2>{c['capHeading']}</h2><p>{c['capCopy']}</p></div>
<div class="bm-caps">{caps}</div>
</section>
<hr class="bm-rule">
<section class="wrap bm-section" id="method">
<div class="bm-head"><h2>{c['methodHeading']}</h2><p>{c['methodCopy']}</p></div>
<div class="bm-method"><h3>{c['methodTitle']}</h3><ol>{steps}</ol>
<div class="bm-caveat"><p><b>{c['caveatTitle']}</b></p>{caveats}</div>
<p class="bm-footnote">{c['footnote']}</p></div>
</section>
<hr class="bm-rule">
<section class="wrap bm-section" id="benchmark-faq"><h2>{c['faqHeading']}</h2><div class="bm-faq">{faqs}</div></section>
</main>
<footer class="wrap bm-footer"><p>{c['footer']}</p><p><a href="{github}">{'源码' if lang == 'zh' else 'Source'}</a> · <a href="{home}{'zh/' if lang == 'zh' else ''}">{c['backHome']}</a></p></footer>
</body></html>'''
