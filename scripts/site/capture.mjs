#!/usr/bin/env node
/** Real component captures. Desktop Vite :5299; mobile Vite :5288, both
 * started with `--host 127.0.0.1` (localhost resolves to ::1 and times out).
 * Uses the isolated headless Patchright bundled with patchwright-cli.
 * PATCHRIGHT_MODULE can point to another installed Patchright or
 * playwright-core package.
 */
import {execFileSync} from 'node:child_process';
import {readFileSync, mkdirSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
const require = createRequire(import.meta.url);
const pkg = process.env.PATCHRIGHT_MODULE
  || `${execFileSync('npm',['root','-g'],{encoding:'utf8'}).trim()}/patchwright-cli/node_modules/patchright`;
const {chromium} = require(pkg);
const root = fileURLToPath(new URL('../../', import.meta.url));
const out = `${root}docs/screenshots/current`;
mkdirSync(out, {recursive:true});
const DESKTOP = 'http://127.0.0.1:5299';
const MOBILE = 'http://127.0.0.1:5288';
// A cold Vite server compiles the whole app on first load.
const COLD = {timeout:120000};
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
    const co = c.coordinator;
    const ctx = await browser.newContext({viewport:{width:1280,height:800},deviceScaleFactor:2,locale:lang==='zh'?'zh-CN':'en-US',colorScheme:'light'});
    await ctx.addInitScript(lang => {
      // Also runs in the sandboxed material iframes, which have no storage.
      if (window !== window.top) return;
      localStorage.setItem('mock-store:theme','light');
      localStorage.setItem('mock-store:viewMode','history');
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
    p.on('pageerror', e => failed.push('pageerror: '+e.message));
    async function settle() {
      await p.mouse.move(0,0);
      await p.evaluate(() => document.activeElement?.blur());
      await p.waitForFunction(() => [...document.images].every(i => i.complete && i.naturalWidth > 0));
      await p.evaluate(async () => {await document.fonts.ready;});
      await p.waitForTimeout(400);
    }
    async function capture(name, opts = {}) {
      await settle();
      const text = await p.locator('body').innerText();
      if (lang === 'en' && /[㐀-鿿]/u.test(text)) throw new Error(`Mixed-language ${name}: ${text}`);
      await p.screenshot({animations:'disabled',path:`${out}/${name}-${lang}.png`,...opts});
    }
    const taskRow = () => p.getByText(c.tasks[0][0],{exact:true}).first();

    // work — the home page: tasks across projects grouped by what they need
    // from you, the new-task composer holding a brief on the right.
    await p.goto(`${DESKTOP}/?mock&website=${lang}`);
    await p.waitForFunction(brief => [...document.querySelectorAll('textarea')].some(t=>t.value===brief),c.brief,COLD);
    await p.addScriptTag({content:'window.__mock_fleet_ask()'});
    await p.getByText(co.subtasks[0][0]).first().waitFor();
    await capture('work');

    // review — the decision card inline at the end of the task's conversation:
    // which sub-task asked, the question, the options. Its review material is
    // filed in the rail, not inside the card.
    await p.setViewportSize({width:1280,height:1000});
    await taskRow().click();
    await p.getByText(c.options[0],{exact:true}).waitFor();
    await p.getByText(c.artifacts[0],{exact:true}).first().waitFor();
    await settle();
    const cardClip = await p.evaluate(({option, from}) => {
      // The card's header ("from sub-task …") sits above the body holding the
      // options, so climb from the option label until both are inside.
      const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      let card = null;
      while (walker.nextNode()) if (walker.currentNode.textContent.trim() === option) { card = walker.currentNode.parentElement; break; }
      while (card && !((card.textContent || '').includes(option) && (card.textContent || '').includes(from))) card = card.parentElement;
      if (!card) throw new Error('Inline decision card not found');
      const r = card.getBoundingClientRect();
      if (r.bottom > innerHeight) throw new Error('Pinned decision card overflows the viewport');
      return {x:Math.max(0,r.x-16),y:Math.max(0,r.y-16),width:r.width+32,height:Math.min(innerHeight-Math.max(0,r.y-16),r.height+32)};
    }, {option:c.options[0], from:co.subtasks[0][0]});
    await capture('review',{clip:cardClip});

    // relay — the whole task page: main conversation, its sub-tasks, steps
    // and filed material in the rail.
    await p.setViewportSize({width:1280,height:800});
    await capture('relay');

    // agents — a sub-task opened from the rail, on a different agent tool
    // than the coordinator; the developer view names the tool and model.
    await p.getByText(co.subtasks[1][0],{exact:true}).first().click();
    await p.getByText(co.subtasks[1][1]).first().waitFor();
    await p.getByRole('button',{name:/^(Developer view|开发者视图)/}).click();
    await p.waitForFunction(() => /gpt-5\.6|GPT-5\.6/i.test(document.body.innerText));
    await capture('agents');

    // results — the Library: everything every task produced, in one place.
    await p.getByRole('button',{name:/^(Library|资料)$/}).first().click();
    await p.getByText(c.artifacts[7],{exact:true}).waitFor();
    await p.getByText(c.docs[0],{exact:true}).waitFor();
    await p.getByText(c.artifacts[4],{exact:true}).click();
    await p.waitForFunction(() => [...document.querySelectorAll('img')].some(i=>i.src.includes('artifact_blob')&&i.complete&&i.naturalWidth>0),{},{timeout:30000});
    await capture('results');

    // mobile — the phone opens on what needs you, with the sub-task's card.
    await p.setViewportSize({width:430,height:740});
    await p.goto(`${MOBILE}/?mock&website=${lang}`);
    await p.getByText(c.options[1],{exact:true}).waitFor(COLD);
    await capture('mobile');
    await ctx.close();
    if(failed.length) throw new Error(JSON.stringify(failed));
    console.log({lang,screenshots:6,failedResources:0});
  }
} catch(error) {
 for(const context of browser.contexts()) for(const p of context.pages()) { console.error('CAPTURE PAGE',p.url(),(await p.locator('body').innerText()).slice(0,3000)); await p.screenshot({path:`${tmpdir()}/fleet-capture-failure.png`}); }
 throw error;
} finally {await browser.close();}
