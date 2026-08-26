/**
 * Telling a pane's own wheel gesture apart from one belonging to a scrollable
 * box inside it.
 *
 * `wheel` bubbles, so a gesture aimed at anything in the conversation pane
 * also arrives at the pane's own listeners. The transcript is full of boxes
 * that scroll on their own — subagent result cards, diffs, thinking blocks,
 * tool inputs, all `max-height` + `overflow-y: auto` — and a listener that
 * treats every wheel it receives as the pane's own will act on gestures the
 * reader never aimed at it.
 */

/** Below this, a box is treated as having nothing to scroll. */
const NO_OVERFLOW_PX = 2;
/** Below this, the end stop is close enough that the box can't absorb more. */
const AT_LIMIT_PX = 4;

/**
 * Whether a scrollable box *between* the wheel's target and `pane` is about to
 * absorb this gesture.
 *
 * Room in the wheel's own direction is what decides it. A box scrolled to its
 * end stops absorbing and the browser chains the rest to the pane — from there
 * the gesture really is the pane's, so this returns false and the caller acts
 * on it as usual.
 */
export function nestedScrollerWillConsume(pane: HTMLElement, ev: WheelEvent): boolean {
  const delta = ev.deltaY;
  if (!delta) return false;

  let node = ev.target instanceof Node ? ev.target : null;
  while (node && node !== pane) {
    if (node instanceof HTMLElement) {
      const overflowY = window.getComputedStyle(node).overflowY;
      if (overflowY === "auto" || overflowY === "scroll" || overflowY === "overlay") {
        const overflow = node.scrollHeight - node.clientHeight;
        const room = delta > 0 ? overflow - node.scrollTop : node.scrollTop;
        if (overflow > NO_OVERFLOW_PX && room > AT_LIMIT_PX) return true;
      }
    }
    node = node.parentNode;
  }
  return false;
}
