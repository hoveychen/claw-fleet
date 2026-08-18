import { AbilityConstant, ConfigurationConstant, Configuration, UIAbility, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';
import { unifiedDataChannel } from '@kit.ArkData';
import { initTheme, setSystemDark } from '../common/Theme';
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

  /// 别的 app 分享进来时，内容不在 want 参数里，而是放进 UDMF：want 只带一把
  /// key，真正的数据要用那把 key 去 unifiedDataChannel 查。冷启动走 onCreate、
  /// app 已在后台走 onNewWant —— 两条都要接，否则「app 开着时分享」会静默丢。
  ///
  /// 结果存进 AppStorage 而不是直接喂 WebView：这一刻页面可能还没加载好，由
  /// WebShell 自己挑时机取走。nonce 用来区分「同样的文本又分享了一次」。
  private stashSharedWant(want: Want): void {
    if (want.action !== 'ohos.want.action.sendData') {
      return;
    }
    const params = want.parameters as Record<string, Object> | undefined;
    const key = params?.['ability.want.params.udKey'];
    if (typeof key !== 'string' || key.length === 0) {
      return;
    }
    const options: unifiedDataChannel.Options = {
      intention: unifiedDataChannel.Intention.DATA_HUB,
      key: key,
    };
    unifiedDataChannel.queryData(options).then((list: Array<unifiedDataChannel.UnifiedData>) => {
      const texts: string[] = [];
      for (const data of list) {
        for (const rec of data.getRecords()) {
          const plain = rec as unifiedDataChannel.PlainText;
          if (plain.textContent !== undefined && plain.textContent.length > 0) {
            texts.push(plain.textContent);
          }
        }
      }
      if (texts.length === 0) {
        return;
      }
      AppStorage.setOrCreate('sharedText', texts.join('\n\n'));
      const n: number = AppStorage.get<number>('sharedNonce') ?? 0;
      AppStorage.setOrCreate('sharedNonce', n + 1);
    }).catch((e: Error) => {
      hilog.warn(DOMAIN, TAG, 'share query failed: %{public}s', e.message);
    });
  }

  onNewWant(want: Want, _launchParam: AbilityConstant.LaunchParam): void {
    this.stashSharedWant(want);
  }

  onCreate(want: Want, _launchParam: AbilityConstant.LaunchParam): void {
    this.stashSharedWant(want);
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
  }
}
