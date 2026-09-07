/** With docs served on :5290, verify localized images and uncropped layouts. */
import {execFileSync} from 'node:child_process';
import {createRequire} from 'node:module';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PATCHRIGHT_MODULE || `${execFileSync('npm',['root','-g'],{encoding:'utf8'}).trim()}/patchwright-cli/node_modules/patchright`);
const browser=await chromium.launch({headless:true,channel:'chrome'});
try {for(const lang of ['en','zh']) for(const width of [1440,390]) {
 const p=await browser.newPage({viewport:{width,height:1000},locale:lang==='zh'?'zh-CN':'en-US'});
 const failed=[];p.on('response',r=>{if(r.status()>=400)failed.push(r.url());});
 await p.goto(`http://127.0.0.1:5290/${lang==='zh'?'zh/':''}?lang=${lang}`);
 await p.locator('.phone').scrollIntoViewIfNeeded();
 for(const n of [0,1,2]) {
  await p.locator(`#tab-${n}`).click();
  await p.locator(`#panel-${n}`).scrollIntoViewIfNeeded();
  await p.waitForFunction(n=>{const i=document.querySelector(`#panel-${n} img`);return i.complete&&i.naturalWidth>0;},n);
  const box=await p.locator(`#panel-${n} img`).evaluate(i=>({w:i.clientWidth,h:i.clientHeight,pw:i.parentElement.clientWidth,ph:i.parentElement.clientHeight,src:i.getAttribute('src')}));
  if(box.w===0||box.w>box.pw+2||box.h>box.ph+2||!new URL(box.src,'http://localhost').pathname.endsWith(`-${lang}.png`))throw Error(JSON.stringify(box));
  await p.locator(`#panel-${n}`).screenshot({path:`.playwright-cli/site-${lang}-${width}-${n}.png`});
 }
 await p.locator('.phone').scrollIntoViewIfNeeded();
 await p.waitForFunction(()=>{const i=document.querySelector('.phone img');return i.complete&&i.naturalWidth>0;});
 const dimensions=await p.locator('.product-window img,.phone img').evaluateAll(imgs=>imgs.map(i=>({src:i.getAttribute('src'),width:i.clientWidth,height:i.clientHeight,parentWidth:i.parentElement.clientWidth,parentHeight:i.parentElement.clientHeight})));
 if(dimensions.some(i=>i.width>i.parentWidth+2||i.height>i.parentHeight+2))throw Error(JSON.stringify(dimensions));
 if(await p.evaluate(()=>document.documentElement.scrollWidth>innerWidth)) throw Error('Horizontal overflow');

 if(await p.locator('.capability-group li').count()!==48)throw Error('Incomplete feature catalogue');
 await p.locator('.catalogue-toggle').click();
 if(await p.locator('.capability-group[open]').count()!==8)throw Error('Catalogue did not expand');
 if(await p.locator('.capability-group summary span').evaluateAll(items=>items.some(i=>getComputedStyle(i).transform!=='none')))throw Error('Rotated catalogue counts');
 await p.locator('#capabilities').screenshot({path:`.playwright-cli/features-${lang}-${width}.png`});
 await p.locator('.catalogue-toggle').click();
 if(await p.locator('.capability-group[open]').count()!==0)throw Error('Catalogue did not collapse');
 await p.locator('.mobile-section').screenshot({path:`.playwright-cli/site-${lang}-${width}-phone.png`});
 if(await p.locator('.screenshot-open').evaluateAll(links=>links.some(a=>a.href!==a.querySelector('img').src || a.target!=='_blank')))throw Error('Broken full-size image link');
 await p.locator('#tab-0').click();
 await p.evaluate(()=>scrollTo(0,0));
 await p.screenshot({path:`.playwright-cli/full-${lang}-${width}.png`,fullPage:true});
 if(failed.length)throw Error(JSON.stringify(failed));
 console.log({lang,width,images:dimensions,failedResources:failed.length});await p.close();
}} finally {await browser.close();}
