import { window } from '@kit.ArkUI';
import { common } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0000;
const TAG = 'fleet';

/// App-wide safe-area insets (vp), published to AppStorage as `safeTop` /
/// `safeBottom` and forwarded by WebShell into mobile-web's --safe-top /
/// --safe-bottom CSS variables (a WebView cannot read env(safe-area-inset-*)).
///
/// dumpLayout showed the content root is framework-inset on TOP only ([0,133]…) —
/// the framework does NOT reserve the bottom gesture bar (content runs to the screen
/// bottom y=2848), so bottom bars / input docks must reserve it themselves from
/// getWindowAvoidArea(TYPE_SYSTEM).bottomRect.
///
/// CAVEAT: that measurement is from 2026-07-15, when this was still an ATOMIC
/// SERVICE (LMR-AL00, density 3.25). We have since switched to bundleType "app";
/// the top-only-inset behaviour has NOT been re-verified in the app form. The
/// atomic-service menu capsule (close / more) that used to force a measured 104vp
/// right inset on every header does not exist in a normal app, so those constants
/// are gone.

let bound = false;

function publish(win: window.Window): void {
  const sys = win.getWindowAvoidArea(window.AvoidAreaType.TYPE_SYSTEM);
  const topVp = px2vp(sys.topRect.height);
  const bottomVp = px2vp(sys.bottomRect.height);
  AppStorage.setOrCreate('safeTop', topVp);
  AppStorage.setOrCreate('safeBottom', bottomVp);
  // Full-rect calibration log: every rect of TYPE_SYSTEM (x/y/w/h in px) so the
  // capsule geometry can be re-derived from the API later if a rect ever exposes
  // it. grep 'safe-area'.
  const t = sys.topRect;
  const b = sys.bottomRect;
  const l = sys.leftRect;
  const r = sys.rightRect;
  hilog.info(DOMAIN, TAG,
    'safe-area TYPE_SYSTEM top[%{public}d,%{public}d,%{public}d,%{public}d] bottom[%{public}d,%{public}d,%{public}d,%{public}d] left[%{public}d,%{public}d,%{public}d,%{public}d] right[%{public}d,%{public}d,%{public}d,%{public}d]',
    t.left, t.top, t.width, t.height, b.left, b.top, b.width, b.height,
    l.left, l.top, l.width, l.height, r.left, r.top, r.width, r.height);
}

/// Resolve the current window's system avoid area, publish it, and (once) keep it
/// live across rotation / fold. Call from EntryAbility.onWindowStageCreate.
export async function initSafeArea(context: common.UIAbilityContext): Promise<void> {
  try {
    const win = await window.getLastWindow(context);
    publish(win);
    if (!bound) {
      win.on('avoidAreaChange', (data: window.AvoidAreaOptions) => {
        if (data.type === window.AvoidAreaType.TYPE_SYSTEM) {
          publish(win);
        }
      });
      bound = true;
    }
  } catch (e) {
    hilog.warn(DOMAIN, TAG, 'safe-area init failed: %{public}s', JSON.stringify(e));
  }
}
