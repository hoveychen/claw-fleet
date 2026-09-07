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
const assets = {};
for (const lang of ['en','zh']) for (let i=0;i<8;i++) {
  const ext = i===3||i===4?'md':'html';
  assets[`website-${lang}-${i}`] = {contentType:ext==='md'?'text/markdown':'text/html', body:readFileSync(new URL(`./fixtures/${lang}/${i}.${ext}`,import.meta.url),'utf8')};
}
const browser = await chromium.launch({headless:true,channel:'chrome'});
try {
  for (const lang of ['en','zh']) {
    const c = scenes[lang];
    const ctx = await browser.newContext({viewport:{width:1280,height:820},deviceScaleFactor:2,locale:lang==='zh'?'zh-CN':'en-US',colorScheme:'light'});
    await ctx.addInitScript(lang => {
      localStorage.setItem('mock-store:theme','light');
      localStorage.setItem('mock-store:viewMode','history');
      localStorage.setItem('mock-store:sidebar-collapsed','true');
      localStorage.setItem('fleet-lang',lang);
    }, lang);
    await ctx.route('**/artifact_blob?*', route => {
      const id = new URL(route.request().url()).searchParams.get('id');
      if (!assets[id]) throw new Error('Missing fixture '+id);
      return route.fulfill({status:200,...assets[id]});
    });
    const p = await ctx.newPage();
    const failed = [];
    p.on('response', r => {if(r.status()>=400) failed.push(r.url()+': '+r.status());});
    async function capture(name, clip) {
      await p.evaluate(() => document.fonts.ready);
      const text = await p.locator('body').innerText();
      if (lang === 'en' && /[\u3400-\u9fff]/u.test(text)) throw new Error(`Mixed-language ${name}: ${text}`);
      await p.screenshot({animations:'disabled',path:`${out}/${name}-${lang}.png`,clip});
    }
    await p.goto(`http://localhost:5299/?mock&website=${lang}`);
    await p.getByText(c.tasks[0][0],{exact:true}).first().click();
    await p.getByRole('heading',{name:lang==='en'?'The launch package is ready':'发布资料已经备齐'}).waitFor();
    await p.locator('h2').filter({hasText:lang==='en'?'The launch package is ready':'发布资料已经备齐'}).click();
    await capture('work',{x:74,y:0,width:1206,height:820});
    await p.addScriptTag({content:'window.__mock_fleet_ask()'});
    await p.getByText(c.options[0],{exact:true}).waitFor();
    await p.getByText(c.reviewTitle,{exact:true}).first().waitFor();
    const panel = p.locator('[class*="panel_with_detail_"]').first();
    await panel.screenshot({animations:'disabled',path:`${out}/review-${lang}.png`});
    await p.setViewportSize({width:1000,height:820});
    await p.goto(`http://localhost:5299/?mock&website=${lang}`);
    await p.getByText(c.tasks[0][0],{exact:true}).first().waitFor();
    await p.locator('nav button').filter({has:p.locator('svg.lucide-package')}).click();
    await p.getByText(c.artifacts[7],{exact:true}).waitFor();
    await p.waitForFunction(() => document.querySelectorAll('iframe').length >= 6);
    // Wait for all real preview bodies, not only iframe mount.
    for (const frame of p.frames().slice(1)) await frame.locator('h1').waitFor();
    await capture('results',{x:74,y:0,width:926,height:690});
    await p.setViewportSize({width:430,height:740});
    await p.goto(`http://localhost:5288/?mock&website=${lang}`);
    await p.getByText(c.options[1],{exact:true}).waitFor();
    await capture('mobile');
    await ctx.close();
    if(failed.length) throw new Error(JSON.stringify(failed));
    console.log({lang,screenshots:4,failedResources:0});
  }
} finally {await browser.close();}
