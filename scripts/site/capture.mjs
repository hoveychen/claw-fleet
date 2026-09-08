#!/usr/bin/env node
/** Real component captures. Desktop Vite :5299; mobile Vite :5288.
 * Uses the isolated headless Patchright bundled with patchwright-cli.
 * PATCHRIGHT_MODULE can point to another installed Patchright package.
 */
import {execFileSync} from 'node:child_process';
import {readFileSync, mkdirSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
const require = createRequire(import.meta.url);
const globalRoot = execFileSync('npm',['root','-g'],{encoding:'utf8'}).trim();
const {chromium} = require(process.env.PATCHRIGHT_MODULE || `${globalRoot}/patchwright-cli/node_modules/patchright`);
const root = fileURLToPath(new URL('../../', import.meta.url));
const out = `${root}docs/screenshots/current`;
mkdirSync(out, {recursive:true});
const scenes = JSON.parse(readFileSync(new URL('./fixtures/scenes.json',import.meta.url),'utf8'));
const assetManifest = JSON.parse(readFileSync(new URL('./fixtures/assets.json',import.meta.url),'utf8'));
const assets = {};
for (const lang of ['en','zh']) for (const a of assetManifest[lang]) {
  assets[a.id] = {contentType:a.mime,body:readFileSync(new URL(`./fixtures/${lang}/${a.name}`,import.meta.url))};
}
const browser = await chromium.launch({headless:true,channel:'chrome'});
try {
  for (const lang of ['en','zh']) {
    const c = scenes[lang];
    const ctx = await browser.newContext({viewport:{width:1280,height:720},deviceScaleFactor:2,locale:lang==='zh'?'zh-CN':'en-US',colorScheme:'light'});
    await ctx.addInitScript(lang => {
      localStorage.setItem('mock-store:theme','light');
      localStorage.setItem('mock-store:viewMode','history');
      localStorage.setItem('mock-store:sidebar-collapsed','true');
      localStorage.setItem('fleet-lang',lang);
    }, lang);
    await ctx.route('**/artifact_blob?*', route => {
      const id = new URL(route.request().url()).searchParams.get('id');
      if (!assets[id]) throw new Error('Missing fixture '+id);
      const a = assets[id];
      const range = /^bytes=(\d+)-(\d*)$/.exec(route.request().headers().range || '');
      if (range) {
        const start=Number(range[1]), end=range[2]?Math.min(Number(range[2]),a.body.length-1):a.body.length-1;
        return route.fulfill({status:206,contentType:a.contentType,body:a.body.subarray(start,end+1),headers:{'content-range':`bytes ${start}-${end}/${a.body.length}`,'accept-ranges':'bytes'}});
      }
      return route.fulfill({status:200,...a});
    });
    const p = await ctx.newPage();
    const failed = [];
    p.on('response', r => {if(r.status()>=400) failed.push(r.url()+': '+r.status());});
    async function capture(name, clip) {
      await p.evaluate(async () => {await document.fonts.ready; await Promise.all([...document.images].map(i=>i.decode()));});
      const text = await p.locator('body').innerText();
      if (lang === 'en' && /[\u3400-\u9fff]/u.test(text)) throw new Error(`Mixed-language ${name}: ${text}`);
      await p.screenshot({animations:'disabled',path:`${out}/${name}-${lang}.png`,clip});
    }
    await p.goto(`http://localhost:5299/?mock&website=${lang}`);
    await p.waitForFunction(brief => [...document.querySelectorAll('textarea')].some(t=>t.value===brief),c.brief);
    await p.mouse.move(0,0);
    const formClip = await p.evaluate(() => {
      document.activeElement?.blur();
      const textarea = document.querySelector('textarea');
      const heading = [...document.querySelectorAll('h3')].find(h => /^(New Session|新建会话)$/.test(h.textContent.trim()));
      if (!heading || !textarea) throw new Error('Missing populated task form');
      if (textarea.scrollHeight > textarea.clientHeight + 2) throw new Error('Brief is clipped');
      const r = heading.parentElement.parentElement.getBoundingClientRect();
      return {x:Math.max(0,r.x-12),y:Math.max(0,r.y-12),width:r.width+24,height:r.height+24};
    });
    await p.waitForFunction(() => [...document.querySelectorAll('img')].every(i => i.complete && i.naturalWidth > 0));
    await capture('work',formClip);
    await p.addScriptTag({content:'window.__mock_fleet_ask()'});
    await p.getByText(c.options[0],{exact:true}).waitFor();
    await p.getByText(c.reviewTitle,{exact:true}).first().waitFor();
    await p.waitForFunction(() => [...document.querySelectorAll('iframe')].some(f => f.getBoundingClientRect().height > 400));
    const panel = p.locator('[class*="panel_with_detail_"]').first();
    await panel.screenshot({animations:'disabled',path:`${out}/review-${lang}.png`});
    await p.setViewportSize({width:1000,height:820});
    await p.goto(`http://localhost:5299/?mock&website=${lang}`);
    await p.getByText(c.tasks[0][0],{exact:true}).first().waitFor();
    await p.locator('nav button').filter({has:p.locator('svg.lucide-package')}).click();
    await p.getByText(c.artifacts[7],{exact:true}).waitFor();
    await p.waitForFunction(() => document.querySelectorAll('[data-ready="1"]').length >= 7,{},{timeout:30000});
    await p.waitForFunction(() => [...document.querySelectorAll('img')].filter(i=>i.src.includes('artifact_blob')).every(i=>i.complete&&i.naturalWidth>0));
    await capture('results',{x:74,y:0,width:926,height:690});
    // Session board with the three harnesses side by side — the landing page's
    // multi-agent section needs one frame where claude/codex/dsh are all live.
    await p.setViewportSize({width:1000,height:900});
    await p.locator('nav button').filter({has:p.locator('svg.lucide-menu')}).click();
    await p.getByText(c.tasks[0][0],{exact:true}).first().waitFor();
    await p.waitForFunction(() => document.querySelectorAll('[data-session-id]').length >= 8);
    await p.mouse.move(0,0);
    const agentsClip = await p.evaluate(() => {
      const cards = [...document.querySelectorAll('[data-session-id]')];
      if (cards.length < 8) throw new Error('Too few session cards: ' + cards.length);
      const shown = cards.slice(0, 8).map(el => el.innerText).join(' ');
      for (const model of ['Opus', 'gpt-5.6', 'deepseek/']) {
        if (!shown.includes(model)) throw new Error('Agent board missing ' + model);
      }
      const rects = cards.map(el => el.getBoundingClientRect());
      const tops = [...new Set(rects.map(r => Math.round(r.top)))].sort((a, b) => a - b).slice(0, 4);
      const last = tops[tops.length - 1];
      const bottom = Math.max(...rects.filter(r => Math.round(r.top) === last).map(r => r.bottom));
      return {x: 74, y: 0, width: 926, height: Math.round(bottom) + 14};
    });
    await capture('agents', agentsClip);
    await p.setViewportSize({width:430,height:740});
    await p.goto(`http://localhost:5288/?mock&website=${lang}`);
    await p.getByText(c.options[1],{exact:true}).waitFor();
    await capture('mobile');
    await ctx.close();
    if(failed.length) throw new Error(JSON.stringify(failed));
    console.log({lang,screenshots:5,failedResources:0});
  }
} catch(error) {
 for(const context of browser.contexts()) for(const p of context.pages()) { console.error('CAPTURE PAGE',p.url(),await p.locator('body').innerText()); await p.screenshot({path:'/tmp/fleet-capture-failure.png'}); }
 throw error;
} finally {await browser.close();}
