import { window } from '@kit.ArkUI';
import { common } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0000;
const TAG = 'fleet';

/// Right-side inset (vp) reserved on every top-of-screen header so its right-hand
/// controls clear the system menu capsule (close / more) that the atomic service
/// framework floats over the top-right corner.
///
/// The capsule is NOT reported by any public avoid-area API, so this is a measured
/// constant. On-device (LMR-AL00, density 3.25, dumpLayout 2026-07-15): the content
/// root is framework-inset to y=133px (below the status bar), and the capsule floats
/// at screen x≈961–1210 — i.e. ~97vp in from the right edge. Reserving ~100vp of
/// right padding puts the header's right controls just left of the capsule. The
/// header title (left / centered) never reaches the capsule, so it is untouched.
export const CAPSULE_INSET_RIGHT: number = 104;

/// Uniform top-header height (vp) so header content vertically centers on the
/// system capsule (close / more buttons), which floats a little lower than a
/// default-height header's text baseline. On-device (density 3.25): content root
/// is framework-inset to screen y=133, the capsule center sits at screen y≈223 —
/// i.e. content-relative ≈90px ≈27.7vp. A 56vp header (content centered) puts the
/// text center at ≈28vp, aligning it with the capsule and dropping the bottom
/// divider a touch lower, as requested. Applied to all 6 top headers.
export const HEADER_HEIGHT: number = 56;

/// App-wide bottom safe-area inset (vp), published to AppStorage as `safeBottom`.
///
/// dumpLayout shows the atomic service content root is framework-inset on TOP only
/// ([0,133]…) — the framework does NOT reserve the bottom gesture bar (content runs
/// to the screen bottom y=2848). So bottom bars / input docks must reserve it
/// themselves; `safeBottom` carries getWindowAvoidArea(TYPE_SYSTEM).bottomRect. The
/// top status bar needs no app inset (the framework already handles it), so we do
/// NOT apply a top inset — that was double-reserving and pushed every header down a
/// full row (fixed 2026-07-15).
///
/// `safeTop` is still published for diagnostics / fallback but is intentionally not
/// applied to any header.

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
