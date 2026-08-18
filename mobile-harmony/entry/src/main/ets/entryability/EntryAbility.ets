import { AbilityConstant, ConfigurationConstant, Configuration, UIAbility, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';
import { SharedInbox, SharedItem } from '../common/SharedInbox';
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
    // 分享扩展可能在本应用不在前台时投递过内容，每次回前台都取一次。
    this.drainSharedInbox();
  }

  /// 分享由独立的 ShareExtAbility 进程接收并写进 SharedInbox（preferences），
  /// 这里只在前台时取走 —— 两者不同进程，AppStorage 传不过来。
  private drainSharedInbox(): void {
    SharedInbox.take(this.context).then((item: SharedItem | null) => {
      hilog.info(DOMAIN, TAG, 'share-diag drain item=%{public}s',
        item === null ? '(empty)' : ('text=' + item.text.length + ' files=' + item.fileNames.length));
      if (item === null) {
        return;
      }
      // 图片本体还没有通道送进 WebView（那要给拦截器加一条沙箱映射），先把
      // 文件名并进正文 —— 用户至少看得见自己分享了什么，而不是一片空白。
      const parts: string[] = [];
      if (item.text.length > 0) { parts.push(item.text); }
      if (item.fileNames.length > 0) { parts.push('[共享文件] ' + item.fileNames.join(', ')); }
      AppStorage.setOrCreate('sharedText', parts.join('\n\n'));
      const n: number = AppStorage.get<number>('sharedNonce') ?? 0;
      AppStorage.setOrCreate('sharedNonce', n + 1);
      hilog.info(DOMAIN, TAG, 'share-diag published to AppStorage nonce=%{public}d', n + 1);
    }).catch((e: Error) => {
      hilog.warn(DOMAIN, TAG, 'drain share inbox failed: %{public}s', e.message);
    });
  }

  onCreate(_want: Want, _launchParam: AbilityConstant.LaunchParam): void {
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
    this.drainSharedInbox();
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
