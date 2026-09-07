#!/usr/bin/env python3
"""Generate two indexable, JS-independent landing pages: python3 scripts/site/build.py."""
from pathlib import Path
from html import escape

ROOT = Path(__file__).resolve().parents[2]
COPY = {
'en': dict(
 title='Claw Fleet — A home for your coding agents', description='Bring Claude Code and Codex into one workspace. Follow every task, answer decisions in one inbox, and keep work moving from your phone. Free and open source.',
 nav=['Explore','On your phone','Download'], language='中文', skip='Skip to content',
 headline='More gets done.<br>Less needs you.', intro='A home for your coding agents.', lede='Bring Claude Code and Codex together. See what’s moving, step in when it matters, and take your work with you.', cta='Download Claw Fleet', secondary='Explore the workspace', meta='Free & open source. Your agents, your machine.',
 tabs=['See the work','Make a decision','Keep it moving'], sample='Product walkthrough · Example data', caption='The big picture, without opening another terminal.',
 decisionHeading='One decision. Back to work.', decisionCopy='Questions from your agents meet in one inbox. See the context, choose a direction, and let the task continue.',
 example='Interactive example', task='Refresh the checkout page', agent='Codex is waiting for your direction', question='How should we roll out the new checkout?', answers=['Start with a small rollout','Release to everyone'], answerDetail=['Try it with 10% of traffic first.','Make the new checkout the default.'], send='Send answer', sent='Answer sent. The agent can continue.', reset='Try again', simulated='This example stays in your browser. No agent or deployment is started.',
 relayHeading='A new context.<br>The same task.', relayCopy='When an agent registers a handoff, Fleet starts the next session with its brief and plan. Follow the chain without piecing the story together yourself.', relayStages=['Plan the work','Build and verify','Pick up the next step'], relayDetails=['The task and checklist live with the project.','Progress and decisions stay visible.','A handoff carries the brief into a fresh session.'], relayNote='Session 01 → Handoff → Session 02',
 sectionHeading='Stay with the work.<br>Skip the window hopping.', sectionText='A few tasks or a busy afternoon: keep the important parts in view.',
 features=[('Know what needs you.','See which sessions are working, waiting, or ready for your attention. Open the timeline when you want the details.'),('Catch the expensive detour.','Follow token usage and estimated spend as tasks run. Interrupt a session when it heads in the wrong direction.'),('Keep what you learned.','Return to plans, reports and a searchable project wiki. The next session starts with more than a blank chat.')],
 mobileHeading='Step away.<br>Stay in the loop.', mobileCopy='Your desk doesn’t have to be the place every decision happens. Pair your phone, open your inbox, and give your agents the next direction.', mobilePoints=['Answer questions and review plans.','Check progress or launch a task.','Connect by QR code from the desktop app.'], mobileCta='How to connect your phone', mobileCaption='Mobile web interface · Example data',
 downloadHeading='Make room for<br>your next idea.', downloadCopy='Get Claw Fleet for your computer. Connect the coding agents you already use.', downloadLabel='Download', source='Download source', globalSource='GitHub Releases', chinaSource='China mirror', sourceNote='GitHub downloads are available now. A China mirror will appear here when it is enabled.', sourceReady='Version {version} · China mirror available. GitHub is also available below.', fallback='GitHub alternative',
 platforms=[('macOS','Apple Silicon & Intel','Universal installer · .pkg'),('Windows','x64','Desktop installer · .exe'),('Linux','x64 / ARM64','Run the workspace in your browser')], allReleases='Release notes & all downloads', linuxHelp='On Linux, download the binary for your processor, then run:', linuxAfter='Open http://127.0.0.1:4571 in your browser. For ARM64, replace x64 with arm64 in both commands.',
 startHeading='From install to your first task.', steps=[('Connect your agent','Install and sign in to Claude Code or Codex first. Open Fleet and use Settings to configure the agents you want to run.'),('Give it something to do','Choose a project, describe the task, and pick your model. Follow progress in the workspace.'),('Bring your phone along','Enable Mobile in the desktop app and scan the pairing QR code. Keep your computer and Fleet running while you connect remotely.')],
 faqHeading='A few things to know.', faqs=[('Is Claw Fleet a coding model?','Fleet brings your coding agents together. You still use Claude Code or Codex to do the coding, with their own account and model access.'),('What does it cost?','Claw Fleet is free and open source under AGPL-3.0. Your coding agent’s subscription or API usage is billed separately. Spend shown in Fleet is an estimate based on session usage.'),('Does my computer need to stay on?','Yes, for the desktop workflow shown here. Your agents run on your computer, so it and Fleet must remain running for your phone to reach them.'),('Where does my data go?','Fleet reads local session files for monitoring. Coding agents connect to their own providers. Remote phone access uses a relay; you can also self-host the relay. Features that call a model use the provider you configure.'),('Do I need a mobile app?','You can use the mobile web interface: enable Mobile in Fleet and scan the QR code. Push notifications depend on your browser, OS and permissions; some devices require adding the site to the home screen.'),('What if GitHub downloads are slow?','Use the China mirror when it is offered in the download section. If it is not listed, it has not been enabled yet. GitHub Releases remains the official alternative.')],
 closing='You set the direction.<br>Your agents take it from there.', footer='An independent, open-source project. Not affiliated with OpenAI or Anthropic.', sourceCode='Source code', license='License', screenshotAlt='Claw Fleet sessions view showing agent status, task progress and estimated usage', mobileAlt='Claw Fleet mobile decision inbox', guide='Project guide',
),
'zh': dict(
 title='Claw Fleet — 把 AI 编程助手，带到一个工作台', description='把 Claude Code 和 Codex 放进同一个工作台。看清任务进度、集中处理决定，离开电脑也能用手机接手。免费、开源，支持 macOS、Windows 和 Linux。',
 nav=['认识 Fleet','手机接手','下载'], language='English', skip='跳至正文',
 headline='让事情往前走。<br>让自己腾出手。', intro='你的 AI 编程助手，有了共同的工作台。', lede='把 Claude Code 和 Codex 放在一起。看清进度，在需要时做决定，离开电脑也能接着干。', cta='下载 Claw Fleet', secondary='看看怎么用', meta='免费开源。用你的助手，在你的电脑上运行。',
 tabs=['看清任务','做个决定','接着往下做'], sample='产品导览 · 示例数据', caption='不用来回切终端，也能看清全局。',
 decisionHeading='做个决定，事情继续。', decisionCopy='助手的问题汇到同一个收件箱。看明白上下文，选一个方向，任务就能继续往前。', example='可交互示例', task='改版结算页面', agent='Codex 正在等你确定方向', question='新版结算页，该怎样上线？', answers=['先小范围上线','直接全量上线'], answerDetail=['先向 10% 的流量开放，观察效果。','让所有用户使用新的结算流程。'], send='发送选择', sent='选择已发送，助手可以继续了。', reset='再试一次', simulated='此示例仅在浏览器中演示，不会启动助手或执行发布。',
 relayHeading='换个上下文。<br>接着同一件事。', relayCopy='助手登记交接后，Fleet 会带着交接说明和计划启动下一段会话。任务的来龙去脉，不用靠你重新拼起来。', relayStages=['把工作写进计划','动手实现与验证','接住下一步'], relayDetails=['任务和清单留在项目里。','进展与决定始终有迹可循。','交接说明随新会话一起带过去。'], relayNote='会话 01 → 交接 → 会话 02',
 sectionHeading='专注手上的事。<br>少在窗口间奔忙。', sectionText='从两三个任务，到忙碌的一整天，重要的事都在眼前。', features=[('一眼知道，谁在等你。','哪些会话在执行、哪些需要你回答，一处看清。想了解细节，点开时间线就能接着看。'),('及时发现，哪条路走贵了。','跟着任务查看用量与预估费用。方向不对时，直接中断会话，把精力用回正事。'),('做过的功课，下次接着用。','计划、报告和可搜索的项目知识库都能随时回看。下一段会话，不必又从一张白纸开始。')],
 mobileHeading='人离开电脑。<br>事情不用等。', mobileCopy='不是每个决定，都要回到桌前再做。配对手机，打开待办，给助手下一步方向。', mobilePoints=['回答问题，查看和确认计划。','了解进度，也能发起新任务。','在桌面端扫码，连接你的手机。'], mobileCta='了解手机连接步骤', mobileCaption='手机网页界面 · 示例数据',
 downloadHeading='给下一个想法，<br>腾个位置。', downloadCopy='在电脑上安装 Claw Fleet，连上你正在使用的编程助手。', downloadLabel='下载', source='下载线路', globalSource='GitHub 官方发布', chinaSource='国内镜像', sourceNote='目前可从 GitHub 下载。国内镜像开通后会在这里提供。', sourceReady='{version} · 国内镜像已开通，也可使用下方 GitHub 备用下载。', fallback='GitHub 备用下载', platforms=[('macOS','Apple Silicon 与 Intel','通用安装包 · .pkg'),('Windows','x64','桌面安装包 · .exe'),('Linux','x64 / ARM64','在浏览器中使用完整工作台')], allReleases='版本说明与全部下载', linuxHelp='Linux：按处理器选择文件，下载后运行：', linuxAfter='在浏览器打开 http://127.0.0.1:4571。ARM64 请把两条命令中的 x64 都替换为 arm64。',
 startHeading='装好，就开始第一件事。', steps=[('连上你的编程助手','先安装并登录 Claude Code 或 Codex。打开 Fleet，在设置中配置你要使用的助手。'),('交给它一件事','选择项目，描述任务，再选好模型。在工作台里跟进过程，需要做决定时再介入。'),('把手机也带上','在桌面端开启「Mobile」，扫描配对二维码。远程使用时，保持电脑和 Fleet 运行。')], faqHeading='你可能还想知道。', faqs=[('Claw Fleet 自己是编程模型吗？','Fleet 把编程助手放到同一个工作台。实际写代码的仍然是 Claude Code 或 Codex，需要你自己的账号和模型访问权限。'),('需要付费吗？','Claw Fleet 免费开源，采用 AGPL-3.0 协议。编程助手的订阅或 API 用量由相应服务商另外计费。Fleet 中的费用显示是根据会话用量估算的。'),('电脑需要一直开着吗？','这里介绍的桌面使用方式需要。助手运行在你的电脑上，手机远程连接时，电脑和 Fleet 都需要保持运行。'),('我的数据会去哪里？','Fleet 通过本机会话文件监看任务。编程助手会连接各自的服务商；手机远程访问经由中继，也支持自建中继。需要调用模型的功能会使用你配置的服务商。'),('手机需要装 App 吗？','可以直接用手机网页：在 Fleet 中开启 Mobile，再扫描二维码。推送取决于浏览器、系统和通知权限，部分设备需要先将网页添加到主屏幕。'),('GitHub 下载很慢怎么办？','下载区提供国内镜像时，切换到国内线路即可。若没有这条线路，说明镜像尚未开通；GitHub 官方发布仍可作为下载来源。')], closing='你来定方向。<br>让助手接着往前。', footer='独立开源项目，与 OpenAI、Anthropic 无隶属关系。', sourceCode='源代码', license='开源协议', screenshotAlt='Claw Fleet 会话总览，展示助手状态、任务进度与预估用量', mobileAlt='Claw Fleet 手机端的决策收件箱', guide='项目指南',
)}

ASSETS = [('claw-fleet-macos.pkg','macOS'),('claw-fleet-windows-x64-setup.exe','Windows'),('fleet-linux-x64','x64'),('fleet-linux-arm64','ARM64')]
GITHUB='https://github.com/hoveychen/claw-fleet'

def build(lang, c):
    base = '../' if lang == 'zh' else './'
    other = '../index.html' if lang == 'zh' else 'zh/index.html'
    def dl(name, label):
        url=f'{GITHUB}/releases/latest/download/{name}'
        return f'<div class="download-link"><a class="button" data-asset="{name}" href="{url}">{label}<span aria-hidden="true">↓</span></a><a class="fallback" href="{url}" hidden>{c["fallback"]}</a></div>'
    rows=''
    for i,(name,arch,desc) in enumerate(c['platforms']):
        links=dl(*ASSETS[i]) if i<2 else dl(*ASSETS[2])+dl(*ASSETS[3])
        rows+=f'<div class="download-row"><div class="platform"><img src="{base}icon-{["apple","windows","linux"][i]}.svg" width="28" height="28" alt=""><h3>{name}</h3></div><div class="architecture"><strong>{arch}</strong><span>{desc}</span></div><div class="download-actions">{links}</div></div>'
    features=''.join(f'<article><h3>{h}</h3><p>{p}</p></article>' for h,p in c['features'])
    steps=''.join(f'<li><h3>{h}</h3><p>{p}</p></li>' for h,p in c['steps'])
    faq=''.join(f'<details><summary>{h}<span aria-hidden="true">+</span></summary><p>{p}</p></details>' for h,p in c['faqs'])
    answers=''.join(f'<label class="choice"><input type="radio" name="rollout" value="{i}" {"checked" if i==0 else ""}><span><strong>{h}</strong><small>{p}</small></span></label>' for i,(h,p) in enumerate(zip(c['answers'],c['answerDetail'])))
    relay=''.join(f'<li><span class="step-dot">{i+1}</span><div><h4>{h}</h4><p>{p}</p></div></li>' for i,(h,p) in enumerate(zip(c['relayStages'],c['relayDetails'])))
    return f'''<!doctype html>
<html lang="{'zh-CN' if lang=='zh' else 'en'}">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{c['title']}</title><meta name="description" content="{escape(c['description'],quote=True)}">
<meta name="color-scheme" content="light"><meta property="og:title" content="{c['title']}"><meta property="og:description" content="{escape(c['description'],quote=True)}"><meta property="og:type" content="website"><meta property="og:image" content="https://hoveychen.github.io/claw-fleet/hero.png"><meta name="twitter:card" content="summary_large_image">
<link rel="alternate" hreflang="en" href="{base}index.html"><link rel="alternate" hreflang="zh-CN" href="{base}zh/index.html"><link rel="alternate" hreflang="x-default" href="{base}index.html">
<link rel="icon" href="{base}icon.png"><link rel="stylesheet" href="{base}site.css"><script src="{base}site.js" defer></script>
</head>
<body data-locale="{lang}">
<a class="skip" href="#main">{c['skip']}</a>
<header class="header"><a class="brand" href="{base}{'zh/' if lang=='zh' else ''}"><img src="{base}icon.png" width="32" height="32" alt="">Claw Fleet</a><nav aria-label="{'主导航' if lang=='zh' else 'Main navigation'}"><a class="nav-explore" href="#explore">{c['nav'][0]}</a><a class="nav-mobile" href="#mobile">{c['nav'][1]}</a><a class="language" href="{other}" lang="{'en' if lang=='zh' else 'zh-CN'}" hreflang="{'en' if lang=='zh' else 'zh-CN'}">{c['language']}</a><a class="nav-download" href="#download">{c['nav'][2]}<span aria-hidden="true"> ↓</span></a></nav></header>
<main id="main">
<section class="hero wrap"><h1>{c['headline']}</h1><div class="hero-copy"><p class="intro">{c['intro']}</p><p>{c['lede']}</p><a class="button primary" href="#download">{c['cta']}<span aria-hidden="true">↓</span></a><a class="text-link" href="#explore">{c['secondary']} <span aria-hidden="true">↗</span></a><small>{c['meta']}</small></div></section>
<section class="showcase wrap" id="explore" aria-label="{c['nav'][0]}">
<div class="showcase-toolbar"><div class="tabs" aria-label="{c['sample']}">{''.join(f'<a id="tab-{i}" class="tab" href="#panel-{i}">{label}</a>' for i,label in enumerate(c['tabs']))}</div><span class="sample">{c['sample']}</span></div>
<div id="panel-0" class="demo-panel"><div class="screenshot-stage"><img src="{base}screenshots/01_gallery.png" width="2880" height="1800" fetchpriority="high" alt="{c['screenshotAlt']}"></div><p class="caption">{c['caption']} <span>Claude Code / Codex</span></p></div>
<div id="panel-1" class="demo-panel"><div class="interactive-stage"><div class="stage-copy"><h2>{c['decisionHeading']}</h2><p>{c['decisionCopy']}</p><span class="example-label">{c['example']}</span></div><form class="decision-demo"><div class="demo-heading"><span class="status-dot"></span>{c['task']}</div><p class="agent-label">{c['agent']}</p><fieldset><legend>{c['question']}</legend>{answers}</fieldset><button type="submit" class="button primary">{c['send']}</button><p class="sent-message" role="status" data-message="{c['sent']}"></p><button class="reset" type="reset" hidden>{c['reset']}</button></form></div><p class="caption">{c['simulated']}</p></div>
<div id="panel-2" class="demo-panel"><div class="interactive-stage relay-stage"><div class="stage-copy"><h2>{c['relayHeading']}</h2><p>{c['relayCopy']}</p><span class="example-label">{c['example']}</span></div><div class="relay-demo"><p class="relay-top">{c['relayNote']}</p><ol>{relay}</ol></div></div><p class="caption">{c['meta']}</p></div>
</section>
<section class="overview wrap"><div class="section-heading"><h2>{c['sectionHeading']}</h2><p>{c['sectionText']}</p></div><div class="feature-columns">{features}</div></section>
<section class="mobile-section wrap" id="mobile"><div class="mobile-art"><div class="phone"><img src="{base}screenshots/02_mobile_decisions.png" width="1170" height="2532" loading="lazy" alt="{c['mobileAlt']}"></div><p>{c['mobileCaption']}</p></div><div class="mobile-copy"><h2>{c['mobileHeading']}</h2><p>{c['mobileCopy']}</p><ul>{''.join(f'<li>{p}</li>' for p in c['mobilePoints'])}</ul><a class="text-link" href="#getting-started">{c['mobileCta']} <span aria-hidden="true">↗</span></a></div></section>
<section class="download-section" id="download"><div class="wrap"><div class="section-heading"><h2>{c['downloadHeading']}</h2><p>{c['downloadCopy']}</p></div><div class="download-source"><label for="download-source">{c['source']}</label><select id="download-source"><option value="github">{c['globalSource']}</option></select><p id="source-note" data-ready="{c['sourceReady']}" data-china="{c['chinaSource']}">{c['sourceNote']}</p></div><div class="downloads">{rows}</div><a class="text-link release-link" href="{GITHUB}/releases">{c['allReleases']} <span aria-hidden="true">↗</span></a><details class="linux-help"><summary>{c['linuxHelp']}<span aria-hidden="true">+</span></summary><pre><code>chmod +x fleet-linux-x64\n./fleet-linux-x64 webui</code></pre><p>{c['linuxAfter']}</p></details></div></section>
<section id="getting-started" class="getting-started wrap"><h2>{c['startHeading']}</h2><ol>{steps}</ol></section>
<section class="faq wrap"><h2>{c['faqHeading']}</h2><div>{faq}</div></section>
<section class="closing wrap"><h2>{c['closing']}</h2><a class="button primary" href="#download">{c['cta']}<span aria-hidden="true">↓</span></a></section>
</main>
<footer class="wrap"><div class="footer-top"><a class="brand" href="#main"><img src="{base}icon.png" width="28" height="28" alt="">Claw Fleet</a><div><a href="{GITHUB}">{c['sourceCode']}</a><a href="{GITHUB}/blob/main/README.md">{c['guide']}</a><a href="{GITHUB}/blob/main/LICENSE">{c['license']}</a><a class="language" href="{other}">{c['language']}</a></div></div><p>{c['footer']}</p></footer>
</body></html>'''

if __name__ == '__main__':
    for lang,c in COPY.items():
        path=ROOT/'docs'/('zh/index.html' if lang=='zh' else 'index.html')
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_text(build(lang,c))
        print(path.relative_to(ROOT))
