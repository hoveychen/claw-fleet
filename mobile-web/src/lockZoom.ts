// The JavaScript fallback layer for locking zoom.
//
// Background: `<meta viewport user-scalable=no/maximum-scale>` has been intentionally
// ignored since Chrome 48+ (to preserve accessibility zoom). `touch-action: pan-x
// pan-y` in index.css is the second line of defense for spec-compliant engines. But
// HarmonyOS NEXT's ArkWeb engine (com.huawei.hmos.browser) doesn't fully respect
// touch-action either — pinch and double-tap still zoom.
//
// `preventDefault` on touch events is a more primitive hook than touch-action — it
// decides whether the browser consumes a gesture as scroll/zoom, the root mechanism
// for Web to control gestures, which almost all engines (including ArkWeb) must
// respect. So we use it as the final fallback:
//   - Multi-finger touchmove → block pinch-zoom (single-finger scroll unaffected)
//   - Two touchend within 300ms → block double-tap-zoom
//   - Safari private gesture* events → block trackpad/old iOS pinch
//   - ctrl+wheel → block desktop trackpad pinch and ctrl+scroll zoom
//
// All registered with `{ passive: false }`, otherwise preventDefault is ineffective.

export function lockZoom(): void {
  document.addEventListener(
    "touchmove",
    (e) => {
      if (e.touches.length > 1) e.preventDefault();
    },
    { passive: false },
  );

  let lastTouchEnd = 0;
  document.addEventListener(
    "touchend",
    (e) => {
      const now = e.timeStamp;
      if (now - lastTouchEnd <= 300) e.preventDefault();
      lastTouchEnd = now;
    },
    { passive: false },
  );

  // Safari/WebKit private pinch events
  for (const type of ["gesturestart", "gesturechange", "gestureend"]) {
    document.addEventListener(type, (e) => e.preventDefault(), {
      passive: false,
    });
  }

  // Desktop trackpad pinch / ctrl+scroll zoom
  document.addEventListener(
    "wheel",
    (e) => {
      if (e.ctrlKey) e.preventDefault();
    },
    { passive: false },
  );
}
