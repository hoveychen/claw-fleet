import { AbilityConstant, ConfigurationConstant, Configuration, UIAbility, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';
import { initTheme, setSystemDark } from '../common/Theme';
import { initI18n } from '../common/I18n';
import { selfTest } from '../common/RelayCrypto';
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

  onCreate(_want: Want, _launchParam: AbilityConstant.LaunchParam): void {
    const cfg = this.context.config;
    const deviceIsZh = (cfg.language ?? 'zh').toLowerCase().startsWith('zh');
    initI18n(deviceIsZh);
    initTheme(isDark(cfg.colorMode));
    // One-shot relay-crypto byte-alignment check against the frozen vector; the
    // verdict lands in hilog (grep 'crypto selfTest'). This is the only runtime
    // proof that AES-GCM interops with the Rust/WebCrypto peers.
    selfTest().then((verdict: string) => {
      hilog.info(DOMAIN, TAG, 'crypto selfTest: %{public}s', verdict);
    });
    // No launch-route parsing: the 元服务 service_notification channel cannot
    // carry tap data (verified 2026-07-14), so a notification tap only foregrounds
    // the app — there is no decision id to route to. See TASKS.md plan
    // harmony-native-client P7.
  }

  onConfigurationUpdate(newConfig: Configuration): void {
    // Follow OS light/dark flips while the app is running.
    setSystemDark(isDark(newConfig.colorMode));
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    windowStage.loadContent('pages/Index', (err) => {
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
  }
}
