#!/usr/bin/env node
/** Capture actual desktop/mobile components with bilingual website fixtures.
 * Start desktop Vite on :5299 and mobile Vite on :5288, then run this script.
 * Dedicated browser contexts, real artifact bytes, explicit crops, no DOM restyling.
 */
import {execFileSync} from 'node:child_process';
import {readFileSync, mkdirSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
const root = fileURLToPath(new URL('../../', import.meta.url));
const out = `${root}docs/screenshots/current`;
mkdirSync(out, {recursive:true});
const scenes = JSON.parse(readFileSync(new URL('./fixtures/scenes.json',import.meta.url),'utf8'));
const pw = (...args) => {
  const result = execFileSync('patchwright-cli',['-s=website-final',...args],{cwd:root,encoding:'utf8'});
  if (result.includes('### Error')) throw new Error(result);
  return result;
};
const assets = {};
for (const lang of ['en','zh']) for (let i=0;i<8;i++) {
  const ext = i===3||i===4?'md':'html';
  assets[`website-${lang}-${i}`] = {contentType:ext==='md'?'text/markdown':'text/html', body:readFileSync(new URL(`./fixtures/${lang}/${i}.${ext}`,import.meta.url),'utf8')};
}
pw('open','about:blank');
try {
for (const lang of ['en','zh']) {
  console.log(pw('run-code',`async page => {
    const c = ${JSON.stringify(scenes[lang])};
    const assets = ${JSON.stringify(assets)};
    const ctx = await page.context().browser().newContext({viewport:{width:1280,height:820},deviceScaleFactor:2,locale:${JSON.stringify(lang==='zh'?'zh-CN':'en-US')},colorScheme:'light'});
    await ctx.addInitScript(() => {
      localStorage.setItem('mock-store:theme','light');
      localStorage.setItem('mock-store:viewMode','history');
      localStorage.setItem('mock-store:sidebar-collapsed','true');
      localStorage.setItem('mock-store:wizard-completed','1');
      localStorage.setItem('fleet-lang',${JSON.stringify(lang)});
    });
    await ctx.route('**/artifact_blob?*', route => {
      const id = new URL(route.request().url()).searchParams.get('id');
      if (!assets[id]) throw new Error('Missing fixture '+id);
      return route.fulfill({status:200,...assets[id]});
    });
    const p = await ctx.newPage();
    const failed = [];
    p.on('response', r => {if(r.status()>=400) failed.push(r.url()+': '+r.status());});
    await p.goto('http://localhost:5299/?mock&website=${lang}');
    await p.getByRole('button',{name:${JSON.stringify(lang==='en'?"Got it, let's go":"好的，开始使用")}}).click();
    await p.getByText(c.tasks[0][0],{exact:true}).first().click();
    await p.getByRole('heading',{name:${JSON.stringify(lang==='en'?'The launch package is ready':'发布资料已经备齐')}}).waitFor();
    await p.evaluate(() => document.fonts.ready);
    await p.screenshot({animations:'disabled',path:${JSON.stringify(out+`/work-${lang}.png`)},clip:{x:74,y:0,width:1206,height:820}});
    await p.addScriptTag({content:'window.__mock_fleet_ask()'});
    await p.getByText(c.options[0],{exact:true}).waitFor();
    const panel = p.locator('[class*="panel_with_detail_"]').first();
    await panel.screenshot({animations:'disabled',path:${JSON.stringify(out+`/review-${lang}.png`)}});
    await p.goto('http://localhost:5299/?mock&website=${lang}');
    await p.getByRole('button',{name:${JSON.stringify(lang==='en'?"Got it, let's go":"好的，开始使用")}}).click();
    await p.locator('nav button').filter({has:p.locator('svg.lucide-package')}).click();
    await p.getByText(c.artifacts[7],{exact:true}).waitFor();
    await p.waitForFunction(() => document.querySelectorAll('iframe').length >= 6);
    await p.waitForTimeout(1600);
    await p.screenshot({animations:'disabled',path:${JSON.stringify(out+`/results-${lang}.png`)},clip:{x:74,y:0,width:1206,height:720}});
    await p.setViewportSize({width:430,height:932});
    await p.goto('http://localhost:5288/?mock&website=${lang}');
    await p.getByText(c.decision,{exact:true}).first().waitFor();
    await p.screenshot({animations:'disabled',path:${JSON.stringify(out+`/mobile-${lang}.png`)}});
    const text = await p.locator('body').innerText();
    if (${JSON.stringify(lang)} === 'en' && /[\\u3400-\\u9fff]/u.test(text)) throw new Error('Mixed-language mobile screenshot: '+text);
    await ctx.close();
    if(failed.length) throw new Error(JSON.stringify(failed));
    return {lang:${JSON.stringify(lang)}, screenshots:4};
  }`));
}
} finally {pw('close');}
