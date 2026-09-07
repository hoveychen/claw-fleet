import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {readFileSync,writeFileSync,statSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PATCHRIGHT_MODULE||`${execFileSync('npm',['root','-g'],{encoding:'utf8'}).trim()}/patchwright-cli/node_modules/patchright`);
const root=fileURLToPath(new URL('./',import.meta.url));
const browser=await chromium.launch({headless:true,channel:'chrome'});
const manifest={};
try {for(const lang of ['en','zh']) {
 const p=await browser.newPage({viewport:{width:900,height:620},deviceScaleFactor:1});
 await p.setContent(readFileSync(`${root}${lang}/packaging-source.html`,'utf8'));await p.screenshot({path:`${root}${lang}/04-packaging.png`});await p.close();
 execFileSync('ffmpeg',['-y','-loglevel','error','-loop','1','-i',`${root}${lang}/04-packaging.png`,'-vf','scale=900:620,zoompan=z=min(zoom+0.0004\\,1.08):d=180:s=900x620:fps=30','-t','6','-c:v','libx264','-preset','fast','-crf','24','-pix_fmt','yuv420p','-movflags','+faststart',`${root}${lang}/06-film.mp4`]);
 const specs=[['00-brief.pdf','application/pdf','pdf'],['01-budget.xlsx','application/vnd.openxmlformats-officedocument.spreadsheetml.sheet','sheet'],['02-launch.pptx','application/vnd.openxmlformats-officedocument.presentationml.presentation','slides'],['03-guide.docx','application/vnd.openxmlformats-officedocument.wordprocessingml.document','doc'],['04-packaging.png','image/png','image'],['05-garden.html','text/html','text'],['06-film.mp4','video/mp4','video'],['07-checklist.md','text/markdown','text']];
 manifest[lang]=specs.map(([name,mime,kind],i)=>({id:`website-${lang}-${i}`,name,mime,kind,sizeBytes:statSync(`${root}${lang}/${name}`).size}));
} } finally {await browser.close();}
writeFileSync(`${root}assets.json`,JSON.stringify(manifest,null,2)+'\n');
console.log('Generated two packaging images, two 6-second films and measured all 16 files.');
