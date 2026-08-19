/**
 * Force WebKit to recompute a scroll container's scrollable extent.
 *
 * The conversation pane intermittently stops scrolling while the DOM insists
 * everything is fine: `scrollHeight` far exceeds `clientHeight`, the element's
 * laid-out box matches its client box, wheel events arrive — and `scrollTop`
 * will not move. Measured in the field (see `scrollFreezeProbe`), with the
 * auto-follow pin provably idle at the time:
 *
 *   verdict=frozen intent=-189 moved=0 room=4557 scrollHeight=5030
 *   clientHeight=469 following=false pins=0
 *
 * Nothing above the compositor can be at fault when the numbers look like
 * that — WebKit is holding a stale scrollable extent for the layer. The user's
 * own workaround was to switch to another session tab and back, which flips
 * `.pane` between `position: absolute` and `relative` and forces the whole
 * subtree through layout again. This does the same thing deliberately and
 * without disturbing what is on screen: drop the element out of its scrolling
 * mode, read a layout property to make the change take effect synchronously,
 * and put it back.
 *
 * Kept separate from its callers so the sequence — and the fact that it
 * restores the reader's position — can be tested directly.
 */

/**
 * Guards against re-entry. Both callers act on signals a revive can itself
 * produce (a ResizeObserver on the very element being restyled, a scroll
 * that lands after the extent is refreshed), so without this a revive can
 * re-trigger itself.
 */
let reviving = false;

/**
 * Rebuild `el`'s scroll layer, preserving `scrollTop`.
 *
 * Returns false when a revive is already in progress, so callers can tell a
 * skipped re-entry from a real one.
 */
export function reviveScrollLayer(el: HTMLElement): boolean {
  if (reviving) return false;
  reviving = true;
  try {
    const top = el.scrollTop;
    // Whatever the stylesheet asked for — restoring a hard-coded "auto" would
    // silently rewrite an element styled `overflow-y: scroll`.
    const authored = el.style.overflowY;

    el.style.overflowY = "hidden";
    // Reading a layout property forces the style change to be applied now.
    // Without it both writes coalesce into one no-op before the next paint and
    // the extent is never recomputed.
    void el.offsetHeight;

    el.style.overflowY = authored;
    void el.offsetHeight;

    // `overflow: hidden` clamps scrollTop to 0, so this is a restore, not a
    // courtesy: losing the reader's place would be worse than the freeze.
    el.scrollTop = top;
    return true;
  } finally {
    reviving = false;
  }
}
