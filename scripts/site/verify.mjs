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
 for(const n of [0,1,2,3]) {
  await p.locator(`#tab-${n}`).click();
  await p.locator(`#panel-${n}`).scrollIntoViewIfNeeded();
  await p.waitForFunction(n=>{const i=document.querySelector(`#panel-${n} img`);return i.complete&&i.naturalWidth>0;},n);
  // currentSrc, not the src attribute: the screenshots are <picture> now, so the
  // attribute is the PNG fallback while the browser actually paints the WebP --
  // checking only the attribute would stop catching a wrong-language image.
  const box=await p.locator(`#panel-${n} img`).evaluate(i=>({w:i.clientWidth,h:i.clientHeight,pw:i.parentElement.clientWidth,ph:i.parentElement.clientHeight,src:i.getAttribute('src'),shown:i.currentSrc}));
  const shownPath=new URL(box.shown,'http://localhost').pathname;
  if(box.w===0||box.w>box.pw+2||box.h>box.ph+2||!new URL(box.src,'http://localhost').pathname.endsWith(`-${lang}.png`)||!new RegExp(`-${lang}\\.(webp|png)$`).test(shownPath))throw Error(JSON.stringify(box));
  await p.locator(`#panel-${n}`).screenshot({path:`.playwright-cli/site-${lang}-${width}-${n}.png`});
 }
 await p.locator('.phone').scrollIntoViewIfNeeded();
 await p.waitForFunction(()=>{const i=document.querySelector('.phone img');return i.complete&&i.naturalWidth>0;});
 // The multi-agent board is the one deliberate exception: at phone width it is
 // upscaled past its frame so the per-card model names stay readable, and the
 // stage scrolls sideways instead. Assert that scroll exists rather than fit.
 const dimensions=await p.locator('.product-window img,.phone img').evaluateAll(imgs=>imgs.filter(i=>!i.closest('.harness-stage')).map(i=>({src:i.getAttribute('src'),width:i.clientWidth,height:i.clientHeight,parentWidth:i.parentElement.clientWidth,parentHeight:i.parentElement.clientHeight})));
 if(dimensions.some(i=>i.width>i.parentWidth+2||i.height>i.parentHeight+2))throw Error(JSON.stringify(dimensions));
 const board=await p.locator('.harness-stage .product-window').evaluate(el=>({scrollW:el.scrollWidth,clientW:el.clientWidth,overflowX:getComputedStyle(el).overflowX,imgW:el.querySelector('img').clientWidth}));
 if(board.imgW<200||board.scrollW<board.imgW-2)throw Error('Agent board clipped: '+JSON.stringify(board));
 if(board.imgW>board.clientW+2&&board.overflowX!=='auto')throw Error('Agent board overflows without scroll: '+JSON.stringify(board));
 if(await p.evaluate(()=>document.documentElement.scrollWidth>innerWidth)) throw Error('Horizontal overflow');

 if(await p.locator('.capability-group li').count()!==54)throw Error('Incomplete feature catalogue');
 await p.locator('.catalogue-toggle').click();
 if(await p.locator('.capability-group[open]').count()!==9)throw Error('Catalogue did not expand');
 if(await p.locator('.capability-group summary span').evaluateAll(items=>items.some(i=>getComputedStyle(i).transform!=='none')))throw Error('Rotated catalogue counts');
 await p.locator('#capabilities').screenshot({path:`.playwright-cli/features-${lang}-${width}.png`});
 await p.locator('.catalogue-toggle').click();
 if(await p.locator('.capability-group[open]').count()!==0)throw Error('Catalogue did not collapse');
 await p.locator('.mobile-section').screenshot({path:`.playwright-cli/site-${lang}-${width}-phone.png`});
 if(await p.locator('.screenshot-open').evaluateAll(links=>links.some(a=>a.href!==a.querySelector('img').src || a.target!=='_blank')))throw Error('Broken full-size image link');
 await p.locator('#tab-0').click();
 await p.evaluate(()=>scrollTo(0,0));
 await p.screenshot({path:`.playwright-cli/full-${lang}-${width}.png`,fullPage:true});

 // Field report page — reached the way a reader reaches it, through the nav entry.
 await p.locator('.nav-benchmark').click();
 await p.waitForURL(/benchmark\.html/);
 const report=await p.evaluate(()=>({charts:document.querySelectorAll('.bm-chart').length,squares:document.querySelectorAll('.bm-units i').length,tables:document.querySelectorAll('.bm-data table').length,days:document.querySelectorAll('.bm-hit').length,tiles:document.querySelectorAll('.bm-tile').length,caps:document.querySelectorAll('.bm-cap').length,faqs:document.querySelectorAll('.bm-faq details').length}));
 if(report.charts!==3||report.squares!==82||report.tables!==3||report.days<28||report.tiles!==4||report.caps!==6||report.faqs!==4)throw Error('Field report incomplete: '+JSON.stringify(report));
 // The curve draws outside its own <svg> box (peak label, axis text), so assert
 // the page — not the element — stays inside the viewport at both widths.
 if(await p.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error('Field report overflows horizontally');
 if(await p.evaluate(()=>{const s=document.querySelector('.bm-svg');const r=s.getBoundingClientRect();return r.width<200||r.height<120;}))throw Error('Daily curve collapsed');
 await p.screenshot({path:`.playwright-cli/benchmark-${lang}-${width}.png`,fullPage:true});

 if(failed.length)throw Error(JSON.stringify(failed));
 console.log({lang,width,images:dimensions,failedResources:failed.length});await p.close();
}} finally {await browser.close();}
