// Publishes the soft keyboard's footprint as CSS variables on <html>.
//
// iOS Safari does not shrink the layout viewport when the keyboard opens; the
// keyboard overlays the page. `100dvh` and `position: fixed; bottom: 0` keep
// pointing at the bottom of the screen, so every bottom-anchored input (the
// session reply pill, the new-task sheet, bottom sheets) ends up under the
// keyboard. iOS then pans the visual viewport trying to reveal the focused
// field, and against a full-screen fixed page that pan lands in the wrong
// place — users reported having to drag the page up and down a few times
// before the input showed.
//
// visualViewport is the only API that reports how much of the screen is still
// visible, so we read it once here and expose:
//   --kb-inset   px between the layout viewport's bottom and the visible bottom
//                (0 with no keyboard, and on Android where the layout viewport
//                already shrinks)
//   --vv-height  px height of the visible area
//   data-kb-open on <html> while the keyboard is up, for rules that should drop
//                resting-state spacing (home-indicator safe area, peek bar gap)

/** Height hidden below the visible area, in layout-viewport px. Sub-pixel
 *  noise and the few px iOS toolbars wobble by are not a keyboard. */
export function keyboardInset(innerHeight: number, vvHeight: number, vvOffsetTop: number): number {
  const inset = Math.round(innerHeight - vvHeight - vvOffsetTop);
  return inset > 40 ? inset : 0;
}

export function installKeyboardInset(): void {
  const vv = window.visualViewport;
  if (!vv) return;
  const root = document.documentElement;
  let last = -1;
  let lastHeight = -1;
  const apply = () => {
    const inset = keyboardInset(window.innerHeight, vv.height, vv.offsetTop);
    const height = Math.round(vv.height);
    // documentElement style changes are observed (Composer remeasures on
    // them), so only write when something actually moved.
    if (inset === last && height === lastHeight) return;
    last = inset;
    lastHeight = height;
    root.style.setProperty("--kb-inset", `${inset}px`);
    root.style.setProperty("--vv-height", `${height}px`);
    if (inset > 0) root.setAttribute("data-kb-open", "");
    else root.removeAttribute("data-kb-open");
  };
  apply();
  vv.addEventListener("resize", apply);
  vv.addEventListener("scroll", apply);
}
