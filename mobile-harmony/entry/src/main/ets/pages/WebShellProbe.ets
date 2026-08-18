import { webview } from '@kit.ArkWeb';
import { common } from '@kit.AbilityKit';

/// P2 探针页 —— 验完删除。
///
/// v3 用 `resource://rawfile/` 做 src + onInterceptRequest，错误零变化：拦截器
/// 不接管内置 scheme。v4 改走 Capacitor 在安卓/iOS 上的同一条路——把 dist 挂在
/// 一个 URL 下由自己供给。
///
/// 用 https 而不是 http：mobile-web 的 relayCrypto.ts 依赖 crypto.subtle 做
/// HKDF，而 Web Crypto 只在安全上下文可用。普通 http 域名下配对握手会直接挂，
/// 那种坑要等到跑通 UI 之后才暴露，不如一开始就避开。
const ORIGIN = 'https://fleet.local/';

@Entry
@Component
struct WebShellProbe {
  private controller: webview.WebviewController = new webview.WebviewController();
  @State private probe: string = 'loading…';
  private hits: number = 0;
  private misses: string = '';

  private mime(path: string): string {
    if (path.endsWith('.js') || path.endsWith('.mjs')) return 'application/javascript';
    if (path.endsWith('.css')) return 'text/css';
    if (path.endsWith('.html')) return 'text/html';
    if (path.endsWith('.json') || path.endsWith('.webmanifest')) return 'application/json';
    if (path.endsWith('.svg')) return 'image/svg+xml';
    if (path.endsWith('.png')) return 'image/png';
    if (path.endsWith('.ico')) return 'image/x-icon';
    if (path.endsWith('.woff2')) return 'font/woff2';
    return 'application/octet-stream';
  }

  private strip(s: string): string {
    let r: string = s;
    const q: number = r.indexOf('?');
    if (q >= 0) r = r.substring(0, q);
    const h: number = r.indexOf('#');
    if (h >= 0) r = r.substring(0, h);
    return r;
  }

  build() {
    Stack({ alignContent: Alignment.Bottom }) {
      Web({ src: ORIGIN + 'index.html', controller: this.controller })
        .javaScriptAccess(true)
        .domStorageAccess(true)
        .fileAccess(true)
        .imageAccess(true)
        .mixedMode(MixedMode.All)
        .runJavaScriptOnDocumentStart([{
          script: `window.__probeErrors = [];
                   window.addEventListener('error', function(e){
                     var t = e.target || {};
                     window.__probeErrors.push(t.src || t.href || e.message || 'err');
                   }, true);`,
          scriptRules: ['*'],
        }])
        .onInterceptRequest((event) => {
          const url: string = event.request.getRequestUrl();
          if (!url.startsWith(ORIGIN)) return null;
          const rel: string = 'web/' + this.strip(url.substring(ORIGIN.length));
          try {
            const ctx = getContext(this) as common.UIAbilityContext;
            const bytes: Uint8Array = ctx.resourceManager.getRawFileContentSync(rel);
            const resp = new WebResourceResponse();
            resp.setResponseData(bytes.buffer as ArrayBuffer);
            resp.setResponseMimeType(this.mime(rel));
            resp.setResponseEncoding('utf-8');
            resp.setResponseCode(200);
            resp.setReasonMessage('OK');
            resp.setResponseIsReady(true);
            this.hits = this.hits + 1;
            return resp;
          } catch (e) {
            // 记下第一个取不到的，比"某处失败了"有用得多
            if (this.misses.length < 80) this.misses = this.misses + rel + ';';
            return null;
          }
        })
        .onPageEnd(() => { this.poll(0); })
        .width('100%')
        .height('100%')

      Text(this.probe)
        .fontSize(10)
        .padding(6)
        .backgroundColor('#dd000000')
        .fontColor(Color.White)
        .width('100%')
    }
    .width('100%')
    .height('100%')
  }

  private poll(n: number): void {
    this.controller.runJavaScript(
      `(function(){
         var r = document.getElementById('root');
         return JSON.stringify({
           mounted: !!(r && r.children.length > 0),
           secure: window.isSecureContext === true,
           subtle: !!(window.crypto && window.crypto.subtle),
           errors: (window.__probeErrors||[]).slice(0,2)
         });
       })()`,
    ).then((r: string) => {
      this.probe = 'try' + n + ' hits=' + this.hits + ' miss=' + this.misses + ' ' + r;
      const ok: boolean = r.indexOf('"mounted":true') >= 0;
      if (!ok && n < 12) {
        setTimeout(() => { this.poll(n + 1); }, 500);
      }
    }).catch((e: Error) => { this.probe = 'probe failed: ' + e.message; });
  }
}
