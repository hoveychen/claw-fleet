import { AbilityConstant, ConfigurationConstant, Configuration, UIAbility, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';
import { initTheme, setSystemDark, themeTokens } from '../common/Theme';
import { initSafeArea } from '../common/SafeArea';
import { initWakeLock } from '../common/WakeLock';

const DOMAIN = 0x0000;
const TAG = 'fleet';

/// COLOR_MODE_LIGHT → light; DARK and NOT_SET both resolve to dark, matching the
/// web client's `color-scheme: dark` default.
function isDark(mode?: ConfigurationConstant.ColorMode): boolean {
  return mode !== ConfigurationConstant.ColorMode.COLOR_MODE_LIGHT;
}

export default class EntryAbility extends UIAbility {
  /// Background freeze suspends JS timers, so a reconnect scheduled while
  /// backgrounded may never fire. Bump a nonce on every foreground; MainShell
  /// @Watch-es it and kicks the relay client if it isn't authed.
  onForeground(): void {
    AppStorage.setOrCreate('fgNonce', Date.now());
  }

  /// 分享内容随 startAbility 的 want 参数一起过来（见 SharePanel）。
  ///
  /// 早先走 preferences 跨进程交接，真机日志里主应用读到的一直是 (empty)：
  /// UIExtension 与 UIAbility 是两个进程，preferences 未必落在同一沙箱路径，
  /// 而且还要赌「extension 写完」早于「主应用读取」。want 参数跟着拉起动作
  /// 一起过去，天然有序，也不依赖沙箱共享。
  private takeSharedFromWant(want: Want): void {
    const params = want.parameters as Record<string, Object> | undefined;
    const text = params?.['fleet.share.text'];
    const rawFiles = params?.['fleet.share.files'];
    const textStr: string = typeof text === 'string' ? text : '';
    const filesStr: string = typeof rawFiles === 'string' ? rawFiles : '';
    hilog.info(DOMAIN, TAG, 'share-diag want text=%{public}d files=%{public}d',
      textStr.length, filesStr.length);
    if (textStr.length === 0 && filesStr.length === 0) {
      return;
    }
    // 文件名列表原样传给 WebShell —— 它会拼成 https://fleet.local/__shared/<name>
    // 交给 web，由拦截器从沙箱把字节喂回去，最终走 web 既有的附件上传链路。
    AppStorage.setOrCreate('sharedFiles', filesStr);
    AppStorage.setOrCreate('sharedText', textStr);
    const n: number = AppStorage.get<number>('sharedNonce') ?? 0;
    AppStorage.setOrCreate('sharedNonce', n + 1);
    hilog.info(DOMAIN, TAG, 'share-diag published nonce=%{public}d', n + 1);
  }

  /// 点推送通知带进来的路由目标。
  ///
  /// relay 把 notify 帧的 url 放进鸿蒙报文的 clickAction.data.fleetUrl(见
  /// harmony_push 的 build_app_notification),Push Kit 再把整个 data 作为 want
  /// 参数交给本 Ability —— 所以这里读的 key 就是 relay 那边写的那个。
  ///
  /// 与分享同一套路数:内容跟着拉起动作走,不依赖任何跨进程存储。
  private takeDeepLinkFromWant(want: Want): void {
    const params = want.parameters as Record<string, Object> | undefined;
    const raw = params?.['fleetUrl'];
    const url: string = typeof raw === 'string' ? raw : '';
    hilog.info(DOMAIN, TAG, 'deeplink-diag want url len=%{public}d', url.length);
    if (url.length === 0) {
      return;
    }
    AppStorage.setOrCreate('deepLinkUrl', url);
    // nonce:连点同一条通知时 url 一模一样，只看 url 的 @Watch 不会再触发。
    const n: number = AppStorage.get<number>('deepLinkNonce') ?? 0;
    AppStorage.setOrCreate('deepLinkNonce', n + 1);
  }

  /// app 已在后台时，分享和推送点击都走这条路而不是 onCreate。
  onNewWant(want: Want, _launchParam: AbilityConstant.LaunchParam): void {
    this.takeSharedFromWant(want);
    this.takeDeepLinkFromWant(want);
  }

  onCreate(want: Want, _launchParam: AbilityConstant.LaunchParam): void {
    this.takeSharedFromWant(want);
    this.takeDeepLinkFromWant(want);
    const cfg = this.context.config;
    // 语言与主题现在都归 web 层：mobile-web 自己读 navigator.language 和
    // prefers-color-scheme。这里只保留 initTheme —— 它喂的是 AppStorage 里的
    // resolvedTheme，原生启动画面/窗口背景仍要用，避免冷启动闪白。
    initTheme(isDark(cfg.colorMode));
    // 原先这里还有一次 relay-crypto 自检；加解密已随 RelayCrypto.ets 一起交回
    // web 层（relayCrypto.ts + crypto.subtle），自检也就没有对象了。
  }

  onConfigurationUpdate(newConfig: Configuration): void {
    // Follow OS light/dark flips while the app is running.
    setSystemDark(isDark(newConfig.colorMode));
    this.applyWindowBackground();
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    windowStage.loadContent('pages/WebShell', (err) => {
      if (err.code) {
        hilog.error(DOMAIN, TAG, 'loadContent failed: %{public}s', JSON.stringify(err));
      }
    });
    // Publish the system avoid-area insets (status-bar band incl. the menu
    // capsule + bottom gesture bar) so every header reserves them. The atomic
    // service window is full-screen immersive, so without this the top-of-screen
    // headers render under the capsule / status bar.
    initSafeArea(this.context);
    // Restore the persisted screen-keep-awake flag onto the fresh window (native
    // mirror of the web client's initWakeLock on startup).
    initWakeLock(this.context);
    this.applyWindowBackground();
  }

  /// 窗口底色。安全区那一条(手势条所在的位置)由**窗口**绘制，不是网页——
  /// ArkUI 组件默认避开安全区，WebView 的底边停在手势条上沿，那条缝露出的
  /// 是窗口默认底色(白)。给 Web 组件设背景、或在 CSS 里改，都够不着它；
  /// 唯一管用的是 setWindowBackgroundColor。
  private applyWindowBackground(): void {
    window.getLastWindow(this.context).then((win: window.Window) => {
      win.setWindowBackgroundColor(themeTokens(AppStorage.get<string>('resolvedTheme') ?? 'dark').bg);
    }).catch((e: Error) => {
      hilog.warn(DOMAIN, TAG, 'set window bg failed: %{public}s', e.message);
    });
  }
}
