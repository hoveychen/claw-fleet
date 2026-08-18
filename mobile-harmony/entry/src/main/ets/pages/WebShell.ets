import { webview } from '@kit.ArkWeb';
import { common } from '@kit.AbilityKit';
import { RelayStore, RelayPairing } from '../common/RelayStore';

/// 鸿蒙端的 WebView 外壳：把打包进 rawfile 的 mobile-web 跑起来，原生只做
/// 三件 web 做不到的事（配对密钥注入、安全区、后续的推送）。
///
/// 为什么是假域名而不是 resource://rawfile/ —— 后者 HTML 能加载、相对路径也
/// 解析对，但子资源取不到内容，且 onInterceptRequest 不接管内置 scheme（两者
/// 都是真机实测出来的，不是查文档）。https 而非 http 也是硬性的：mobile-web
/// 的 relayCrypto 依赖 crypto.subtle，Web Crypto 只在安全上下文可用。
const ORIGIN = 'https://fleet.local/';

@Entry
@Component
struct WebShell {
  private controller: webview.WebviewController = new webview.WebviewController();
  /// 安全区由 SafeArea.ets 发布到 AppStorage；mobile-web 把安全区抽象成了
  /// --safe-top / --safe-bottom 两个 CSS 变量（默认取 env(safe-area-inset-*)，
  /// 而 WebView 里那个 env 拿不到系统值），所以原生覆盖这两个变量即可，
  /// web 侧一行都不用改。
  @StorageProp('safeTop') safeTop: number = 0;
  @StorageProp('safeBottom') safeBottom: number = 0;
  @State private src: string = '';

  aboutToAppear(): void {
    const ctx = getContext(this) as common.UIAbilityContext;
    RelayStore.loadPairing(ctx).then((p: RelayPairing | null) => {
      // 密钥走 URL fragment：mobile-web 的 loadSecretSync 本来就从
      // window.location.hash 读 `#k=`（桌面二维码就是这个形状），所以这里
      // 复用同一条路径，web 侧无需为鸿蒙加任何分支。fragment 也不会进
      // 请求行，拦截器按路径取资源不受影响。
      this.src = p === null ? ORIGIN + 'index.html'
                            : ORIGIN + 'index.html#k=' + p.secret;
    }).catch(() => { this.src = ORIGIN + 'index.html'; });
  }

  private mime(path: string): string {
    if (path.endsWith('.js') || path.endsWith('.mjs')) return 'application/javascript';
    if (path.endsWith('.css')) return 'text/css';
    if (path.endsWith('.html')) return 'text/html';
    if (path.endsWith('.json') || path.endsWith('.webmanifest')) return 'application/json';
    if (path.endsWith('.svg')) return 'image/svg+xml';
    if (path.endsWith('.png')) return 'image/png';
    if (path.endsWith('.ico')) return 'image/x-icon';
    if (path.endsWith('.woff2')) return 'font/woff2';
    if (path.endsWith('.woff')) return 'font/woff';
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

  /// 在页面任何脚本之前写入安全区变量：晚于首帧就会看到一次跳动。
  private insetScript(): string {
    return `(function(){
      var s = document.documentElement.style;
      s.setProperty('--safe-top', '` + this.safeTop.toString() + `px');
      s.setProperty('--safe-bottom', '` + this.safeBottom.toString() + `px');
    })();`;
  }

  build() {
    Column() {
      if (this.src.length > 0) {
        Web({ src: this.src, controller: this.controller })
          .javaScriptAccess(true)
          .domStorageAccess(true)
          .fileAccess(true)
          .imageAccess(true)
          .databaseAccess(true)
          .mixedMode(MixedMode.All)
          .darkMode(WebDarkMode.Auto)
          .runJavaScriptOnDocumentStart([{ script: this.insetScript(), scriptRules: ['*'] }])
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
              return resp;
            } catch (e) {
              return null;
            }
          })
          .width('100%')
          .height('100%')
      }
    }
    .width('100%')
    .height('100%')
  }
}
