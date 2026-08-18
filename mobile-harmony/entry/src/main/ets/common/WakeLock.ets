/// Screen-keep-awake — the native mirror of mobile-web/src/wakeLock.ts. Where the
/// web client holds a Web Screen Wake Lock sentinel, the atomic service asks the
/// window to stay lit via `window.setWindowKeepScreenOn`. The flag persists across
/// launches (AppStorage + PersistentStorage) and is re-applied on window creation.
///
/// Reactivity: the enabled flag lives in AppStorage under `keepScreenOn`;
/// components subscribe with `@StorageProp('keepScreenOn')`. On HarmonyOS the
/// window API is always available, so — unlike the web's `wakeLock.supported`
/// gate — the setting is always offered.

import { common } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0000;
const TAG = 'fleet';
const KEY = 'keepScreenOn';

export function getKeepScreenOn(): boolean {
  return AppStorage.get<boolean>(KEY) ?? false;
}

/// Push the flag down to the main window. Non-fatal on failure — the setting is
/// already persisted, so the next launch's `initWakeLock` re-applies it.
function applyToWindow(context: common.UIAbilityContext, enabled: boolean): void {
  window.getLastWindow(context)
    .then((win: window.Window) => win.setWindowKeepScreenOn(enabled))
    .then(() => {
      hilog.info(DOMAIN, TAG, 'keepScreenOn applied: %{public}s', `${enabled}`);
    })
    .catch((e: Object) => {
      hilog.warn(DOMAIN, TAG, 'keepScreenOn apply failed: %{public}s', JSON.stringify(e));
    });
}

/// Persist the flag and apply it to the window immediately.
export function setKeepScreenOn(context: common.UIAbilityContext, next: boolean): void {
  if (next === getKeepScreenOn()) {
    return;
  }
  AppStorage.setOrCreate(KEY, next);
  applyToWindow(context, next);
}

/// Called from EntryAbility.onWindowStageCreate: restore the persisted flag onto
/// the freshly created window on launch.
export function initWakeLock(context: common.UIAbilityContext): void {
  PersistentStorage.persistProp(KEY, false);
  if (getKeepScreenOn()) {
    applyToWindow(context, true);
  }
}
