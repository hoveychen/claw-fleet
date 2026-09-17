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

  /// Shared content rides in the want parameters of startAbility (see SharePanel).
  ///
  /// Earlier we used preferences for cross-process handoff, but real device logs
  /// showed the main app always read (empty): UIExtension and UIAbility are two
  /// processes, preferences might not land in the same sandbox path, and we'd have
  /// to bet "extension write completes before main app reads". Want parameters
  /// travel with the launch action, so they're naturally ordered and don't depend
  /// on sandbox sharing.
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
    // File name list passes to WebShell as-is — it builds https://fleet.local/__shared/<name>
    // and hands to web; an interceptor feeds bytes back from the sandbox, ending up in
    // web's existing file upload pipeline.
    AppStorage.setOrCreate('sharedFiles', filesStr);
    AppStorage.setOrCreate('sharedText', textStr);
    const n: number = AppStorage.get<number>('sharedNonce') ?? 0;
    AppStorage.setOrCreate('sharedNonce', n + 1);
    hilog.info(DOMAIN, TAG, 'share-diag published nonce=%{public}d', n + 1);
  }

  /// Route target brought in by tapping a push notification.
  ///
  /// The relay puts the notify frame's URL in the HarmonyOS message's
  /// clickAction.data.fleetUrl (see harmony_push's build_app_notification);
  /// Push Kit then passes the whole data as want parameters to this Ability —
  /// so the key we read here is the one the relay writes.
  ///
  /// Same pattern as sharing: content travels with the launch action, depends
  /// on no cross-process storage.
  private takeDeepLinkFromWant(want: Want): void {
    const params = want.parameters as Record<string, Object> | undefined;
    const raw = params?.['fleetUrl'];
    const url: string = typeof raw === 'string' ? raw : '';
    hilog.info(DOMAIN, TAG, 'deeplink-diag want url len=%{public}d', url.length);
    if (url.length === 0) {
      return;
    }
    AppStorage.setOrCreate('deepLinkUrl', url);
    // Nonce: repeatedly tapping the same notification gives identical URL;
    // a @Watch on url alone won't fire again.
    const n: number = AppStorage.get<number>('deepLinkNonce') ?? 0;
    AppStorage.setOrCreate('deepLinkNonce', n + 1);
  }

  /// When app is backgrounded, sharing and notification taps take this path, not onCreate.
  onNewWant(want: Want, _launchParam: AbilityConstant.LaunchParam): void {
    this.takeSharedFromWant(want);
    this.takeDeepLinkFromWant(want);
  }

  onCreate(want: Want, _launchParam: AbilityConstant.LaunchParam): void {
    this.takeSharedFromWant(want);
    this.takeDeepLinkFromWant(want);
    const cfg = this.context.config;
    // Language and theme are now web-layer concerns: mobile-web reads navigator.language
    // and prefers-color-scheme itself. We keep only initTheme — it populates AppStorage's
    // resolvedTheme, still needed by the native splash/window background to avoid a
    // white flash on cold start.
    initTheme(isDark(cfg.colorMode));
    // Earlier there was a relay-crypto self-check here; encrypt/decrypt moved back to
    // the web layer with RelayCrypto.ets (relayCrypto.ts + crypto.subtle), so the
    // self-check is gone.
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

  /// Window background color. The safe-area band (where the gesture bar sits) is
  /// drawn by the **window**, not the web page — ArkUI components avoid the safe
  /// area by default, WebView's bottom stops at the gesture bar top, and the gap
  /// exposes the window's default color (white). Setting a background on the Web
  /// component or changing it in CSS doesn't reach it; only setWindowBackgroundColor works.
  private applyWindowBackground(): void {
    window.getLastWindow(this.context).then((win: window.Window) => {
      win.setWindowBackgroundColor(themeTokens(AppStorage.get<string>('resolvedTheme') ?? 'dark').bg);
    }).catch((e: Error) => {
      hilog.warn(DOMAIN, TAG, 'set window bg failed: %{public}s', e.message);
    });
  }
}
