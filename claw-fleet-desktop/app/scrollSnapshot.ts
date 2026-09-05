/**
 * A raw geometry reading of the transcript scroller.
 *
 * The transcript pane occasionally refuses to scroll — mouse anywhere in it,
 * dead sessions included, wheel does nothing at all, and resizing the window
 * releases it. Three candidates were ruled out by observation (the composer
 * dock swallowing wheels, the auto-follow pin dragging the viewport back, a
 * global overlay eating events), which leaves two that look identical on screen
 * and are fixed in opposite ways:
 *
 * - the scroller genuinely has nothing to scroll (`scrollHeight == clientHeight`
 *   because some height is computed wrong), or
 * - it has room but will not move (`scrollHeight >> clientHeight` and writing
 *   `scrollTop` changes nothing).
 *
 * So this module reads numbers and nothing else. It deliberately draws no
 * conclusion and takes no corrective action: the 2026-08-26 `scrollFreezeProbe`
 * was removed precisely because it classified every gesture as frozen and then
 * "fixed" it, confirming itself 311 times over. The verdict here is made by a
 * human comparing two readings — one taken while stuck, one after the resize
 * that released it.
 */

/** The parts of `window` this reads, named so tests can pass a plain object. */
export interface ViewMetrics {
  innerWidth: number;
  innerHeight: number;
  /** `document.documentElement.clientHeight` — what `100vh` resolves against. */
  documentClientHeight: number;
  /** `visualViewport.height`, absent on hosts without the API. */
  visualViewportHeight?: number;
}

export interface BoxReading {
  /** Class attribute, so a CSS-module hash can be matched back to a rule. */
  className: string;
  top: number;
  height: number;
  clientHeight: number;
  scrollHeight: number;
  overflowY: string;
}

export interface ScrollSnapshot {
  label: string;
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  offsetHeight: number;
  paddingBottom: string;
  /** How far `scrollTop` actually moved when written. 0 = it refused. */
  writeTest: number;
  /** The scroller itself, then each ancestor up to (and including) `.app`. */
  boxes: BoxReading[];
  /** `offsetHeight` of each direct child — the content the scroller holds. */
  childHeights: number[];
  view: ViewMetrics;
}

function readBox(el: HTMLElement): BoxReading {
  const rect = el.getBoundingClientRect();
  return {
    className: el.className || el.tagName.toLowerCase(),
    top: Math.round(rect.top),
    height: Math.round(rect.height),
    clientHeight: el.clientHeight,
    scrollHeight: el.scrollHeight,
    overflowY: getComputedStyle(el).overflowY,
  };
}

/**
 * Write `scrollTop` and put it back, reporting how far it actually moved.
 *
 * The one active step in here, and the only way to tell "no room to scroll"
 * apart from "room, but the box won't move". It restores the original position
 * so taking a reading never changes what the reader is looking at — with the
 * caveat that a box which *does* move has just been scrolled and restored,
 * which is itself a layout read/write and could in principle nudge a stuck
 * scroller loose. That is why the reading is taken before anything else.
 */
export function probeScrollWrite(el: HTMLElement, px = 120): number {
  const before = el.scrollTop;
  // Push away from whichever end we sit at, so the probe is never asking the
  // box to move past a limit it is already against.
  const target = before > px ? before - px : before + px;
  el.scrollTop = target;
  const moved = el.scrollTop - before;
  el.scrollTop = before;
  return moved;
}

/** Ancestors of `el` up to `root`, innermost first, `root` included. */
function ancestry(el: HTMLElement, root: HTMLElement | null): HTMLElement[] {
  const chain: HTMLElement[] = [el];
  let node = el.parentElement;
  while (node) {
    chain.push(node);
    if (node === root || node === document.body) break;
    node = node.parentElement;
  }
  return chain;
}

export function takeScrollSnapshot(
  el: HTMLElement,
  view: ViewMetrics,
  label: string,
): ScrollSnapshot {
  // Before anything reads layout, so a stuck box is asked to move while still
  // in the state the reader is complaining about.
  const writeTest = probeScrollWrite(el);
  return {
    label,
    scrollTop: el.scrollTop,
    scrollHeight: el.scrollHeight,
    clientHeight: el.clientHeight,
    offsetHeight: el.offsetHeight,
    paddingBottom: getComputedStyle(el).paddingBottom,
    writeTest,
    boxes: ancestry(el, document.body).map(readBox),
    childHeights: Array.from(el.children).map((c) => (c as HTMLElement).offsetHeight),
    view,
  };
}

export function currentViewMetrics(): ViewMetrics {
  return {
    innerWidth: window.innerWidth,
    innerHeight: window.innerHeight,
    documentClientHeight: document.documentElement.clientHeight,
    visualViewportHeight: window.visualViewport?.height,
  };
}

/** One-line-per-field text, sized to paste back into a chat unchanged. */
export function formatSnapshot(snap: ScrollSnapshot): string {
  const lines = [
    `[${snap.label}]`,
    `scrollTop=${snap.scrollTop} scrollHeight=${snap.scrollHeight} clientHeight=${snap.clientHeight} offsetHeight=${snap.offsetHeight}`,
    `writeTest=${snap.writeTest} paddingBottom=${snap.paddingBottom}`,
    `view innerW=${snap.view.innerWidth} innerH=${snap.view.innerHeight} docClientH=${snap.view.documentClientHeight} visualH=${snap.view.visualViewportHeight ?? "-"}`,
    `children=[${snap.childHeights.join(", ")}]`,
  ];
  for (const box of snap.boxes) {
    lines.push(
      `  ${box.className} top=${box.top} h=${box.height} client=${box.clientHeight} scroll=${box.scrollHeight} overflowY=${box.overflowY}`,
    );
  }
  return lines.join("\n");
}
