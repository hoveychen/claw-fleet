import { webview } from '@kit.ArkWeb';
import { common } from '@kit.AbilityKit';
import { RelayStore, RelayPairing } from '../common/RelayStore';
import { saveBase64File } from '../common/FileTransfer';
import { setKeepScreenOn } from '../common/WakeLock';
import { shareFile } from '../common/SystemShare';
import { acquirePushToken } from '../common/PushRegister';
import { scanBarcode, scanCore } from '@kit.ScanKit';
import { parsePairingUrl, RelayPairing as ParsedPairing } from '../common/RelayStore';
import { themeTokens } from '../common/Theme';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { fileIo as fs } from '@kit.CoreFileKit';

const DOMAIN_W = 0x0000;
const TAG_W = 'fleet';

/// 鸿蒙端的 WebView 外壳：把打包进 rawfile 的 mobile-web 跑起来，原生只做
/// 三件 web 做不到的事（配对密钥注入、安全区、后续的推送）。
///
/// 为什么是假域名而不是 resource://rawfile/ —— 后者 HTML 能加载、相对路径也
/// 解析对，但子资源取不到内容，且 onInterceptRequest 不接管内置 scheme（两者
/// 都是真机实测出来的，不是查文档）。https 而非 http 也是硬性的：mobile-web
/// 的 relayCrypto 依赖 crypto.subtle，Web Crypto 只在安全上下文可用。
const ORIGIN = 'https://fleet.local/';
/// 分享进来的文件挂在这个前缀下，与 rawfile 里的站点资源区分开。
const SHARED_PREFIX = '__shared/';

/// 交给 web 的文件条目，字段名对齐 shareTarget.ts 的 SharedFile。
interface SharedFileEntry {
  uri: string;
  name: string;
  mimeType: string;
}

/// __fleetShare 的载荷，形状对齐 shareTarget.ts 的 IncomingShare。
interface SharePayload {
  title: string;
  texts: string[];
  files: SharedFileEntry[];
}


/// 暴露给页面的原生桥。只放 web 在 WebView 里确实做不到的事——每多一个方法，
/// mobile-web 就多一分「为鸿蒙写分支」的压力，而那正是我们要消灭的漂移。
class NativeBridge {
  private ctx: common.UIAbilityContext;
  /// 页面请求扫码时回调进页面组件（扫码要动组件状态:src / pairError）。
  private onScanRequested: () => void;

  constructor(ctx: common.UIAbilityContext, onScanRequested: () => void) {
    this.ctx = ctx;
    this.onScanRequested = onScanRequested;
  }

  /// WebView 里 <a download> / createObjectURL 不会触发系统下载，交给
  /// DocumentViewPicker 让用户选位置落盘。
  saveFile(filename: string, base64: string): void {
    saveBase64File(this.ctx, filename, base64).catch(() => {});
  }

  /// 唤起扫码添加设备。壳自己的配对页只在**尚未配对**时可达（build() 只在
  /// src 为空时画它），所以装了 app 的用户没有第二个入口去加第二台机器 ——
  /// 而那是多设备最主要的入口。这里把扫码暴露给页面，页面在设备列表里给出
  /// 一行「扫码添加设备」（mobile-web 的 nativeScan.ts）。
  ///
  /// 回调交给随后那次 WebView 重载：扫到之后走 loadPairing(true)，注入新的
  /// `#k=`，web 侧照常把它收成一台新设备。
  scanPairing(): void {
    this.onScanRequested();
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
  /// Web 组件默认白底。页面还没绘出来的那一瞬、以及任何页面没铺满的边角，
  /// 透出来的就是这层白 —— 深色主题下格外扎眼。跟随 resolvedTheme 兜一层底色。
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  /// EntryAbility 收到系统分享后把内容放这里；nonce 变化即代表「又分享了一次」，
  /// 哪怕文本和上次一模一样。@Watch 是为了让 app 已经开着时的分享也能送达。
  @StorageProp('sharedText') sharedText: string = '';
  /// 分享进来的文件名（换行分隔）。字节由 SharePanel 拷进沙箱，这里只把名字
  /// 拼成 __shared/ 的 URL 交给 web，取字节的活由 onInterceptRequest 干。
  @StorageProp('sharedFiles') sharedFiles: string = '';
  @StorageProp('sharedNonce') @Watch('onSharedChanged') sharedNonce: number = 0;
  /// 点推送通知带进来的路由目标(EntryAbility 从 want 里取,见 takeDeepLinkFromWant)。
  /// nonce 变化即代表「又点了一次」，哪怕目标与上次相同。
  @StorageProp('deepLinkUrl') deepLinkUrl: string = '';
  @StorageProp('deepLinkNonce') @Watch('onDeepLinkChanged') deepLinkNonce: number = 0;
  private webReady: boolean = false;
  /// 设备 push token，取到一次就缓存 —— getToken 会拉起系统通知授权，重复调
  /// 没意义。空串表示还没取到或这台设备取不到（模拟器/未开通推送权益）。
  private pushToken: string = '';
  @State private src: string = '';
  /// null = 还在读配对状态；'' = 未配对（显示配对入口）；其余 = 已配对的 src。
  /// 分三态是因为「还没读完」和「读完了没有」必须区别对待 —— 否则冷启动会
  /// 闪一下配对页再跳走。
  @State private pairChecked: boolean = false;
  @State private pairError: string = '';
  private bridge: NativeBridge = new NativeBridge(
    getContext(this) as common.UIAbilityContext,
    () => { this.startPairing(); },
  );

  aboutToAppear(): void {
    this.loadPairing();
  }

  /// `fresh` = 这次是刚扫完码(抢焦点到那一台);缺省是启动重注(不抢)。
  private loadPairing(fresh: boolean = false): void {
    const ctx = getContext(this) as common.UIAbilityContext;
    RelayStore.loadPairing(ctx).then((p: RelayPairing | null) => {
      this.pairChecked = true;
      // 密钥走 URL fragment：mobile-web 的 consumeHashSecret 本来就从
      // window.location.hash 读 `#k=`（桌面二维码就是这个形状），所以这里
      // 复用同一条路径，web 侧无需为鸿蒙加任何分支。fragment 也不会进
      // 请求行，拦截器按路径取资源不受影响。
      // 未配对时留空 src，让 build() 走原生配对页。
      //
      // 早先这里在 p === null 时也给了 ORIGIN + 'index.html'，结果 src 永远
      // 非空、配对页永远进不去，页面就停在 mobile-web 那张为 PWA 写的引导页
      // 上（它假设用户是扫码打开带密钥的网址，因此没有扫码按钮）——等于没有
      // 任何配对入口。
      //
      // relayUrl 也一起交给页面。以前这里只取 secret，relay 永远是打包时
      // sync-web.sh 烧进 VITE_RELAY_URL 的那个——扫了指向自建 relay 的二维码，
      // app 照样去连官方 relay，配对必然失败且现象只是「一直连不上」。
      // mobile-web 的 consumeHashSecret 认 fragment 里的 &relay=，并把它连同密钥
      // 一起存进那台设备的记录（devices.ts），优先级高于打包值，所以这里给了它
      // 就生效，web 侧不需要鸿蒙分支。多设备之后 relay 归属是**每台设备各自**的，
      // 而这个壳每次加载都重新注入一遍，所以自建 relay 也能在升级后自愈。
      // `boot=1` 说明这是**启动重注**而不是刚扫的码。多设备之后 web 侧把配对
      // 存成一本设备簿,壳每次启动仍会把它存的那一份重新注入一遍;不区分的话
      // 用户在设备列表里切过去的那一台,每次重开 app 都会被这次注入抢回焦点。
      // 刚扫完码那次走 loadPairing(true),不带这个标记 —— 那时抢焦点正是用户要的。
      const boot = fresh ? '' : '&boot=1';
      this.src = p === null
        ? ''
        : ORIGIN + 'index.html#k=' + p.secret + '&relay=' + encodeURIComponent(p.relayUrl) + boot;
    }).catch(() => { this.pairChecked = true; this.src = ''; });
  }

  /// 配对入口。删掉原生 UI 时把 GateView 一起删了，而 mobile-web 的配对页是
  /// 为 PWA 写的——它假设用户是「扫码打开一个带密钥的网址」，壳里没有那个入口，
  /// 于是新装的 app 无路可配对。这里补回最小的一个：扫码写进 RelayStore，
  /// 再让 WebView 带着 #k= 重新加载。
  private async startPairing(): Promise<void> {
    const ctx = getContext(this) as common.UIAbilityContext;
    try {
      const options: scanBarcode.ScanOptions = {
        scanTypes: [scanCore.ScanType.QR_CODE],
        enableMultiMode: false,
        enableAlbum: true,
      };
      const result = await scanBarcode.startScanForResult(ctx, options);
      const pairing: ParsedPairing | null = parsePairingUrl(result.originalValue);
      if (pairing === null) {
        this.pairError = '这个二维码不是 Fleet 的配对码';
        return;
      }
      await RelayStore.savePairing(ctx, pairing);
      this.pairError = '';
      this.loadPairing(true);
    } catch (e) {
      this.pairError = '扫码取消或失败';
    }
  }

  private onSharedChanged(): void {
    this.deliverShare(0);
  }

  private onDeepLinkChanged(): void {
    this.deliverDeepLink();
  }

  /// 把点击目标交给页面。走 decisionDeepLink.ts 约定的 window.__fleetDeepLink ——
  /// 与 __fleetShare / __fleetPushToken 同一套约定,bootScript 已提前占位并排队,
  /// 所以冷启动(点通知拉起 app)时不必跟 React 的 effect 赛跑。
  private deliverDeepLink(): void {
    if (!this.webReady || this.deepLinkUrl.length === 0) {
      hilog.info(DOMAIN_W, TAG_W, 'deeplink-diag deliver skipped ready=%{public}s len=%{public}d',
        this.webReady ? 'yes' : 'no', this.deepLinkUrl.length);
      return;
    }
    this.controller.runJavaScript(
      `(function(){ if(!window.__fleetDeepLink) return 'NO_HOOK';` +
      ` window.__fleetDeepLink(` + JSON.stringify(this.deepLinkUrl) + `); return 'OK'; })()`,
    ).then((r: string) => {
      hilog.info(DOMAIN_W, TAG_W, 'deeplink-diag injected result=%{public}s', r);
      if (r.indexOf('OK') >= 0) {
        // 投递成功即清掉，避免下次回前台又跳一次同样的卡。
        this.deepLinkUrl = '';
      }
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'deeplink-diag inject failed: %{public}s', e.message);
    });
  }

  /// 把分享内容交给页面。走 shareTarget.ts 约定的 window.__fleetShare —— 那是
  /// 给所有原生壳用的通用入口，不是鸿蒙专用分支；Capacitor 侧同样认它。
  ///
  /// 不再重试：bootScript 已在页面任何脚本之前占好 __fleetShare 的位，早到的
  /// 内容先进 __fleetSharePending，shareTarget.ts 注册真 hook 时消费。时序
  /// 依赖因此消失。
  ///
  /// 之所以不靠重试：真机日志里只有 attempt=0，setTimeout 在 ArkTS struct
  /// 方法里根本没回调 —— 那版重试写了等于没写。
  private deliverShare(attempt: number): void {
    if (!this.webReady || (this.sharedText.length === 0 && this.sharedFiles.length === 0)) {
      hilog.info(DOMAIN_W, TAG_W, 'share-diag deliver skipped ready=%{public}s len=%{public}d',
        this.webReady ? 'yes' : 'no', this.sharedText.length);
      return;
    }
    const fileEntries: SharedFileEntry[] = [];
    for (const name of this.sharedFiles.split('\n')) {
      if (name.length === 0) { continue; }
      // 给 web 一个它能直接 fetch 的 URL。shareTarget.ts 里的
      // sharedFilesToFiles 会 fetch 它 → 拦截器从沙箱返回字节 → 变成 File →
      // 走既有的附件上传链路。web 侧因此一行都不用改。
      const entry: SharedFileEntry = {
        uri: ORIGIN + SHARED_PREFIX + name,
        name: name,
        mimeType: this.mime(name),
      };
      fileEntries.push(entry);
    }
    const share: SharePayload = {
      title: '',
      texts: this.sharedText.length > 0 ? [this.sharedText] : [],
      files: fileEntries,
    };
    const payload: string = JSON.stringify(share);
    this.controller.runJavaScript(
      `(function(){ if(!window.__fleetShare) return 'NO_HOOK';` +
      ` window.__fleetShare(` + payload + `); return 'OK'; })()`,
    ).then((r: string) => {
      const ok: boolean = r.indexOf('OK') >= 0;
      hilog.info(DOMAIN_W, TAG_W, 'share-diag injected attempt=%{public}d result=%{public}s',
        attempt, r);
      if (ok) {
        // 投递成功后清掉，避免下次前台又投一遍同样的内容。
        this.sharedText = '';
        this.sharedFiles = '';
        return;
      }
      // 走到这里说明连占位 hook 都不在（bootScript 没跑成），那是别的问题，
      // 重试也救不回来 —— 留日志比空转有用。
      hilog.warn(DOMAIN_W, TAG_W, 'share-diag no hook at all — bootScript may not have run');
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'share-diag inject failed: %{public}s', e.message);
    });
  }

  /// 取设备 push token 并交给页面。原生只负责「拿到 token」，上报 relay 那半边
  /// 归 web（nativePush.ts → push.ts）—— 它才握着 relay 连接、channel token 和
  /// 用户的通知开关状态。元服务时代这里是自己开 WebSocket 报 openId，为此在
  /// ArkTS 里重写了一份 HKDF，两套实现各自漂移；那两个文件已随本次一起删掉。
  ///
  /// 只在已配对时做：未配对时 web 根本没有可注册的 channel，提前弹通知授权
  /// 也只会打扰用户。
  private deliverPushToken(): void {
    if (!this.webReady) { return; }
    if (this.pushToken.length > 0) {
      this.injectPushToken();
      return;
    }
    const ctx = getContext(this) as common.UIAbilityContext;
    acquirePushToken(ctx).then((t: string) => {
      if (t.length === 0) {
        hilog.info(DOMAIN_W, TAG_W, 'push-diag no token on this device — skip register');
        return;
      }
      this.pushToken = t;
      this.injectPushToken();
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'push-diag acquire failed: %{public}s', e.message);
    });
  }

  /// 与 __fleetShare 同一套约定：bootScript 已提前占位 + 排队，所以这里不需要
  /// 重试，也不能靠 setTimeout 重试（ArkTS struct 方法里的 setTimeout 不回调，
  /// 分享那六个坑里已经栽过一次）。
  private injectPushToken(): void {
    this.controller.runJavaScript(
      `(function(){ if(!window.__fleetPushToken) return 'NO_HOOK';` +
      ` window.__fleetPushToken(` + JSON.stringify(this.pushToken) + `); return 'OK'; })()`,
    ).then((r: string) => {
      hilog.info(DOMAIN_W, TAG_W, 'push-diag injected len=%{public}d result=%{public}s',
        this.pushToken.length, r);
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'push-diag inject failed: %{public}s', e.message);
    });
  }

  /// 从沙箱读一个分享进来的文件，喂给 WebView。
  private serveSharedFile(name: string): WebResourceResponse | null {
    try {
      const ctx = getContext(this) as common.UIAbilityContext;
      const path: string = ctx.filesDir + '/shared/' + name;
      const stat = fs.statSync(path);
      const buf = new ArrayBuffer(stat.size);
      const file = fs.openSync(path, fs.OpenMode.READ_ONLY);
      try {
        fs.readSync(file.fd, buf);
      } finally {
        fs.closeSync(file);
      }
      const resp = new WebResourceResponse();
      resp.setResponseData(buf);
      resp.setResponseMimeType(this.mime(name));
      resp.setResponseEncoding('utf-8');
      resp.setResponseCode(200);
      resp.setReasonMessage('OK');
      resp.setResponseIsReady(true);
      hilog.info(DOMAIN_W, TAG_W, 'share-diag served %{public}s bytes=%{public}d', name, stat.size);
      return resp;
    } catch (e) {
      const err = e as Error;
      hilog.warn(DOMAIN_W, TAG_W, 'share-diag serve failed %{public}s: %{public}s', name, err.message);
      return null;
    }
  }

  private mime(path: string): string {
    if (path.endsWith('.js') || path.endsWith('.mjs')) return 'application/javascript';
    if (path.endsWith('.css')) return 'text/css';
    if (path.endsWith('.html')) return 'text/html';
    if (path.endsWith('.json') || path.endsWith('.webmanifest')) return 'application/json';
    if (path.endsWith('.svg')) return 'image/svg+xml';
    if (path.endsWith('.png')) return 'image/png';
    if (path.endsWith('.jpg') || path.endsWith('.jpeg')) return 'image/jpeg';
    if (path.endsWith('.gif')) return 'image/gif';
    if (path.endsWith('.webp')) return 'image/webp';
    if (path.endsWith('.pdf')) return 'application/pdf';
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
      // 先把 __fleetShare 占上位，投递就不必再跟 React 的 useEffect 赛跑。
      //
      // 之前靠「注入失败就 setTimeout 重试」，真机日志里只有 attempt=0 ——
      // ArkTS struct 方法里的 setTimeout 压根没回调，重试等于没写。占位 hook
      // 把内容先收进队列，等 shareTarget.ts 注册真 hook 时一次性消费，时序
      // 依赖就整个消失了。
      if (!window.__fleetShare) {
        window.__fleetSharePending = window.__fleetSharePending || [];
        window.__fleetShare = function(p){ window.__fleetSharePending.push(p); };
      }

      // 推送 token 同理占位：原生取 token 要等系统通知授权，必然晚于首帧，
      // 而 nativePush.ts 的 hook 又挂在 React effect 里 —— 两头都不确定，
      // 于是两头都走队列。
      if (!window.__fleetPushToken) {
        window.__fleetPushTokenPending = window.__fleetPushTokenPending || [];
        window.__fleetPushToken = function(t){ window.__fleetPushTokenPending.push(t); };
      }

      // 点通知拉起 app 时,目标必然早于页面脚本 —— 同样先占位排队。
      if (!window.__fleetDeepLink) {
        window.__fleetDeepLinkPending = window.__fleetDeepLinkPending || [];
        window.__fleetDeepLink = function(u){ window.__fleetDeepLinkPending.push(u); };
      }

      var s = document.documentElement.style;
      // 只给底部。顶部由框架自己 inset 过了 —— SafeArea.ets 的注释记着这笔账:
      // 2026-07-15 就因为应用侧又加了一次 top inset，导致每个页头被整整推下
      // 一行。我第一版照抄注入 safeTop，等于把那个已修的 bug 重新引了回来。
      // safeTop 仍然发布在 AppStorage 里，留作诊断，但不喂给页面。
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
      if (this.pairChecked && this.src.length === 0) {
        Column({ space: 16 }) {
          Text('Fleet').fontSize(28).fontWeight(FontWeight.Bold)
          Text('扫描桌面端 Fleet「移动端」面板里的二维码完成配对')
            .fontSize(14).opacity(0.7).textAlign(TextAlign.Center)
          Button('扫码配对').onClick(() => { this.startPairing(); })
          if (this.pairError.length > 0) {
            Text(this.pairError).fontSize(12).fontColor(Color.Red)
          }
        }
        .width('100%').height('100%')
        .justifyContent(FlexAlign.Center)
        .padding(32)
      } else if (this.src.length > 0) {
        Web({ src: this.src, controller: this.controller })
          .backgroundColor(themeTokens(this.resolvedTheme).bg)
          .javaScriptAccess(true)
          .domStorageAccess(true)
          .fileAccess(true)
          .imageAccess(true)
          .databaseAccess(true)
          .mixedMode(MixedMode.All)
          .darkMode(WebDarkMode.Auto)
          // JS 弹窗必须原生接管。ArkWeb 自己不画 alert/confirm 的弹窗：回调返回
          // false 时「弹窗的处理结果视为取消」，不注册回调等同于此。于是页面里
          // `window.confirm(...)` 恒为 false、`window.alert(...)` 无声消失——
          // 任务页的「停止」按下去没反应，正是因为它的确认框走的这条路。
          //
          // 注册后必须调到 handleConfirm / handleCancel 之一，漏掉会阻塞 render
          // 进程，所以三条出口（确认、取消、点外部/返回键关闭）都要落到 result 上。
          //
          // window.prompt 不接：mobile-web 一处都没用（grep 过），而它需要带
          // 输入框的自定义弹窗，不是 AlertDialog 能顺手覆盖的。真要用时再补。
          .onAlert((event) => {
            if (!event) return false;
            this.getUIContext().showAlertDialog({
              message: event.message,
              autoCancel: false,
              confirm: {
                value: '知道了',
                action: () => { event.result.handleConfirm(); },
              },
              cancel: () => { event.result.handleCancel(); },
            });
            return true;
          })
          .onConfirm((event) => {
            if (!event) return false;
            this.getUIContext().showAlertDialog({
              message: event.message,
              autoCancel: false,
              primaryButton: {
                value: '取消',
                action: () => { event.result.handleCancel(); },
              },
              secondaryButton: {
                value: '确定',
                action: () => { event.result.handleConfirm(); },
              },
              cancel: () => { event.result.handleCancel(); },
            });
            return true;
          })
          // 外链交给系统浏览器，别让它顶掉整个 app。
          //
          // multiWindowAccess 默认 false，此时 ArkWeb 把 target="_blank" 当
          // 「打开本地窗口」处理 —— 知识库文档里点一个外链，这唯一的 WebView
          // 就从 fleet.local 导航走了，Fleet 的界面连同 relay 连接一起被冲掉，
          // 只能靠返回键退回来。开 multiWindowAccess 要配 onWindowNew 再养一个
          // WebView，为几个外链不值当；把非本站的主框架导航整个交给系统才是对的。
          //
          // 只拦主框架的 http/https：知识库的 HTML 文档跑在 data: URI 的沙箱
          // iframe 里（web 侧刻意为之），blob: 则是导出预览在用，两者都必须
          // 原样放行。
          .onLoadIntercept((event) => {
            if (!event) return false;
            const req = event.data;
            if (!req.isMainFrame()) return false;
            const url: string = req.getRequestUrl();
            if (url.startsWith(ORIGIN)) return false;
            if (!url.startsWith('http://') && !url.startsWith('https://')) return false;
            try {
              const ctx = getContext(this) as common.UIAbilityContext;
              ctx.openLink(url, { appLinkingOnly: false }).catch((e: Error) => {
                hilog.warn(DOMAIN_W, TAG_W, 'openLink failed %{public}s', e.message);
              });
            } catch (e) {
              hilog.warn(DOMAIN_W, TAG_W, 'openLink threw: %{public}s', (e as Error).message);
            }
            return true;
          })
          .runJavaScriptOnDocumentStart([{ script: this.bootScript(), scriptRules: ['*'] }])
          .javaScriptProxy({
            object: this.bridge,
            name: 'fleetNative',
            methodList: ['saveFile', 'keepAwake', 'shareContent', 'scanPairing'],
            controller: this.controller,
          })
          .onInterceptRequest((event) => {
            const url: string = event.request.getRequestUrl();
            if (!url.startsWith(ORIGIN)) return null;
            const tail: string = this.strip(url.substring(ORIGIN.length));
            // 分享进来的文件不在 rawfile 里，而在应用沙箱 files/shared/。
            if (tail.startsWith(SHARED_PREFIX)) {
              return this.serveSharedFile(tail.substring(SHARED_PREFIX.length));
            }
            const rel: string = 'web/' + tail;
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
          .onPageEnd(() => {
            // 页面就绪后再补投一次：分享很可能是冷启动带进来的，那时 web 还没
            // 加载完，@Watch 早就烧过了。
            this.webReady = true;
            this.deliverShare(0);
            this.deliverPushToken();
            this.deliverDeepLink();
          })
          .width('100%')
          .height('100%')
      }
    }
    .width('100%')
    .height('100%')
    // 只向下扩展到手势条那一段。ArkUI 组件默认避开安全区，于是 WebView 的
    // 底边停在手势条上沿，那条缝显示的是窗口底色（白），而不是网页——网页
    // 里怎么调 CSS 都够不着它，这正是「导航条是灰的、最下面一条是白的」的
    // 由来。扩展之后网页自己铺到屏幕底部，--safe-bottom 的内边距继续负责
    // 让导航条内容避开手势条。
    //
    // 不扩顶部：顶部框架已经 inset 过（见 SafeArea.ets 里 2026-07-15 那笔
    // 账），再扩会把页头顶到状态栏底下。
    .expandSafeArea([SafeAreaType.SYSTEM], [SafeAreaEdge.BOTTOM])
  }
}
