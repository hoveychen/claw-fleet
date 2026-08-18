import { webview } from '@kit.ArkWeb';
import { common } from '@kit.AbilityKit';
import { RelayStore, RelayPairing } from '../common/RelayStore';
import { saveBase64File } from '../common/FileTransfer';
import { setKeepScreenOn } from '../common/WakeLock';
import { shareFile } from '../common/SystemShare';

/// 鸿蒙端的 WebView 外壳：把打包进 rawfile 的 mobile-web 跑起来，原生只做
/// 三件 web 做不到的事（配对密钥注入、安全区、后续的推送）。
///
/// 为什么是假域名而不是 resource://rawfile/ —— 后者 HTML 能加载、相对路径也
/// 解析对，但子资源取不到内容，且 onInterceptRequest 不接管内置 scheme（两者
/// 都是真机实测出来的，不是查文档）。https 而非 http 也是硬性的：mobile-web
/// 的 relayCrypto 依赖 crypto.subtle，Web Crypto 只在安全上下文可用。
const ORIGIN = 'https://fleet.local/';


/// 暴露给页面的原生桥。只放 web 在 WebView 里确实做不到的事——每多一个方法，
/// mobile-web 就多一分「为鸿蒙写分支」的压力，而那正是我们要消灭的漂移。
class NativeBridge {
  private ctx: common.UIAbilityContext;

  constructor(ctx: common.UIAbilityContext) {
    this.ctx = ctx;
  }

  /// WebView 里 <a download> / createObjectURL 不会触发系统下载，交给
  /// DocumentViewPicker 让用户选位置落盘。
  saveFile(filename: string, base64: string): void {
    saveBase64File(this.ctx, filename, base64).catch(() => {});
  }

  /// navigator.wakeLock 在 ArkWeb 未必可用；不可用时页面回退调这里。
  keepAwake(on: boolean): void {
    setKeepScreenOn(this.ctx, on);
  }

  /// navigator.share 的原生一端，转调 ShareKit 拉起系统分享面板。
  shareContent(filename: string, mimeType: string, base64: string): void {
    shareFile(this.ctx, filename, mimeType, base64).catch(() => {});
  }
}

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
  private bridge: NativeBridge = new NativeBridge(getContext(this) as common.UIAbilityContext);

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

  /// 在页面任何脚本之前跑：安全区要早于首帧写入（否则会看到一次跳动），
  /// 两个补丁也要早于页面代码注册。全部是运行时补丁，mobile-web 源码不动。
  private bootScript(): string {
    return `(function(){
      var s = document.documentElement.style;
      s.setProperty('--safe-top', '` + this.safeTop.toString() + `px');
      s.setProperty('--safe-bottom', '` + this.safeBottom.toString() + `px');

      // 下载：拦 a[download] 的点击，把 blob/data 读成 base64 交给原生落盘。
      // 用捕获阶段，抢在页面自己的 handler 之前。
      document.addEventListener('click', function(ev){
        var a = ev.target && ev.target.closest ? ev.target.closest('a[download]') : null;
        if (!a || !a.href) return;
        ev.preventDefault();
        ev.stopPropagation();
        fetch(a.href).then(function(r){ return r.blob(); }).then(function(b){
          var fr = new FileReader();
          fr.onload = function(){
            var s = String(fr.result || '');
            var i = s.indexOf(',');
            if (i >= 0 && window.fleetNative) {
              window.fleetNative.saveFile(a.getAttribute('download') || 'download', s.slice(i+1));
            }
          };
          fr.readAsDataURL(b);
        }).catch(function(){});
      }, true);

      // 系统分享：ArkWeb 没有 Web Share 时补一个转调 ShareKit 的垫片。
      if (!navigator.share && window.fleetNative) {
        navigator.canShare = function(d){ return !!(d && d.files && d.files.length); };
        navigator.share = function(d){
          var f = d && d.files && d.files[0];
          if (!f) return Promise.reject(new Error('nothing to share'));
          return new Promise(function(res, rej){
            var fr = new FileReader();
            fr.onload = function(){
              var s = String(fr.result || '');
              var i = s.indexOf(',');
              if (i < 0) { rej(new Error('bad data url')); return; }
              window.fleetNative.shareContent(f.name || 'share', f.type || '', s.slice(i+1));
              res();
            };
            fr.onerror = function(){ rej(fr.error); };
            fr.readAsDataURL(f);
          });
        };
      }

      // 保持常亮：ArkWeb 若没有 navigator.wakeLock，就补一个转调原生的垫片，
      // 这样 wakeLock.ts 的既有逻辑无需知道自己跑在鸿蒙里。
      if (!navigator.wakeLock && window.fleetNative) {
        navigator.wakeLock = {
          request: function(){
            window.fleetNative.keepAwake(true);
            return Promise.resolve({
              released: false,
              type: 'screen',
              release: function(){ window.fleetNative.keepAwake(false); return Promise.resolve(); },
              addEventListener: function(){},
              removeEventListener: function(){}
            });
          }
        };
      }
    })();`;
  }

  /// 系统返回手势不会触发页面的 popstate —— 不接这一步，一返回就直接退出 app
  /// 而不是退上一层。mobile-web 的 navStack 每开一层都 pushState，所以
  /// WebView 的历史深度正好等于 UI 层数。
  onBackPress(): boolean {
    if (this.controller.accessBackward()) {
      this.controller.backward();
      return true; // 已消费，不要退出 app
    }
    return false; // 已在根层，交回系统
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
          .runJavaScriptOnDocumentStart([{ script: this.bootScript(), scriptRules: ['*'] }])
          .javaScriptProxy({
            object: this.bridge,
            name: 'fleetNative',
            methodList: ['saveFile', 'keepAwake', 'shareContent'],
            controller: this.controller,
          })
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
