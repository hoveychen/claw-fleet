/**
 * A probe that notices when the conversation pane stops scrolling.
 *
 * The pane freezing mid-session is a real, reported fault that nobody has been
 * able to catch in the act: it appears "at some unknown moment", a release
 * build has no devtools, and by the time it is described the state is gone. So
 * rather than ask the user to reproduce it on command, this watches every
 * scroll gesture and writes one diagnostic line to the debug log the moment a
 * gesture that *should* have moved the viewport doesn't.
 *
 * The whole point is to tell three failure modes apart, because their fixes
 * have nothing in common:
 *
 *  - `no-overflow` — the container believes it has nothing to scroll
 *    (`scrollHeight ≈ clientHeight`). A layout/height fault: the box is sized
 *    wrong, or its content overflowed some ancestor instead of it.
 *  - `yanked` — the viewport did move, and the auto-follow pin
 *    (`SessionDetail`'s ResizeObserver) put it back. A behaviour fault.
 *  - `frozen` — there is room to scroll in the direction asked for, the wheel
 *    events arrive, the pin never ran, and `scrollTop` still doesn't budge.
 *    That is the compositor's scroll layer refusing to move — a WebKit-level
 *    fault no amount of React changes would fix.
 *
 * Verdicts are computed by `classifyScrollAttempt`, kept pure so the
 * discrimination rules can be tested without a browser.
 */

import { reviveScrollLayer } from "./scrollLayerRevive";

/** How the viewport responded to one scroll gesture. */
export type FreezeVerdict =
  /** Moved as asked. Nothing to report. */
  | "ok"
  /** Already against the end stop in that direction — not moving is correct. */
  | "at-limit"
  /** The container reports no scrollable overflow at all. */
  | "no-overflow"
  /** Moved, then auto-follow dragged it back. */
  | "yanked"
  /** Room to move, input arrived, nothing moved, nothing dragged it back. */
  | "frozen";

export interface ScrollAttempt {
  /** Total scrollable overflow: `scrollHeight - clientHeight`. */
  overflow: number;
  /**
   * Remaining scrollable distance in the direction the user asked for. Up is
   * `scrollTop`; down is `scrollHeight - scrollTop - clientHeight`. Without
   * this, every gesture made while already parked at the bottom — which is
   * exactly where the auto-follow leaves the reader — would look like a freeze.
   */
  room: number;
  /** Signed change in `scrollTop` over the gesture. Positive is downward. */
  moved: number;
  /** Signed wheel intent over the gesture. Positive is downward. */
  intent: number;
  /** Whether the auto-follow pin wrote `scrollTop` during the gesture. */
  pinFired: boolean;
}

/** Below this, a container is treated as having nothing to scroll. */
const NO_OVERFLOW_PX = 2;
/** Below this, the end stop is close enough that not moving is expected. */
const AT_LIMIT_PX = 4;
/** Movement under this is indistinguishable from rounding / rubber-banding. */
const MIN_MOVE_PX = 2;

/**
 * Judge one scroll gesture.
 *
 * Order matters: the innocent explanations (no overflow, already at the end
 * stop) are ruled out first, so a gesture is only ever called a fault once
 * there was genuinely somewhere for it to go.
 */
export function classifyScrollAttempt(a: ScrollAttempt): FreezeVerdict {
  if (a.intent === 0) return "ok";
  if (a.overflow <= NO_OVERFLOW_PX) return "no-overflow";
  if (a.room <= AT_LIMIT_PX) return "at-limit";

  const wantedDown = a.intent > 0;
  const movedEnough = Math.abs(a.moved) >= MIN_MOVE_PX;
  const movedAsAsked = wantedDown ? a.moved > 0 : a.moved < 0;

  if (movedEnough && movedAsAsked) return "ok";
  // Either it never moved, or it moved and something put it back. The pin is
  // the only thing in the app that writes scrollTop on its own.
  if (a.pinFired) return "yanked";
  return "frozen";
}

/**
 * Whether a scrollable box *between* the wheel's target and the pane is about
 * to absorb this gesture.
 *
 * `wheel` bubbles, so every gesture aimed at something inside the transcript —
 * a subagent result card, a diff, a thinking block, all of which are
 * `max-height` + `overflow-y: auto` boxes — also arrives at the pane's own
 * listeners. The pane not moving is then the correct outcome, not a freeze:
 * the box ate the scroll. Without this check the probe read exactly that as a
 * frozen pane and drove `scrollTop` itself, so the transcript scrolled in
 * lockstep with the card the reader was actually reading
 * (「滚动里面的内容，外部的滚动条也一起滚」).
 *
 * Room in the wheel's own direction is what decides it. A box scrolled to its
 * end stops absorbing, the browser chains the rest to the pane, and from there
 * the gesture really is the pane's — so the freeze repair has to stay armed for
 * it. Shared with `SessionDetail`'s auto-follow listener, which sits on the
 * same element and has the same reason to ignore these events.
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

/** Verdicts worth writing to the log. The rest are normal scrolling. */
export function isFaultVerdict(v: FreezeVerdict): boolean {
  return v === "no-overflow" || v === "yanked" || v === "frozen";
}

/** How long ago the scroll content last changed height, and by how much. */
export interface ContentGrowth {
  sinceMs: number;
  byPx: number;
}

/** Live state the probe reads at fault time, supplied by `SessionDetail`. */
export interface ProbeContext {
  sessionId: () => string | null;
  status: () => string | null;
  messageCount: () => number;
  isFollowing: () => boolean;
  /** Total auto-follow pin writes since mount. */
  pinCount: () => number;
  /**
   * The content's last height change, or null if it hasn't changed since this
   * transcript was opened.
   *
   * Here to test one hypothesis and nothing else. WebKit bug 150974 —
   * "dynamic content in a scrollable container breaks scrolling" — would
   * explain the freeze, and the prevention layer has exactly the matching gap:
   * `installScrollBoxResizeRevive` watches the scroll box's *own* height, so a
   * transcript appending messages, or markdown / highlighting / images settling
   * over later frames, changes the content height with nothing reviving the
   * layer. If frozen gestures turn out to cluster right after a height change,
   * that gap is worth closing; if they don't, the hypothesis is dead and no one
   * has to guess at it again. Supplied by the caller because the auto-follow
   * pin already observes the content — a second ResizeObserver on the same
   * children would be duplicate machinery.
   */
  contentGrowth: () => ContentGrowth | null;
}

/** Writes one line to the host debug log. Injected so tests stay off Tauri. */
export type DiagSink = (line: string) => void;

/** Gesture is considered over once this long passes with no wheel event. */
const GESTURE_IDLE_MS = 140;
/** At most one report per this window, so a stuck pane can't flood the log. */
const REPORT_COOLDOWN_MS = 30_000;

/**
 * Watch `el` for scroll gestures that don't land, repairing and reporting them.
 *
 * The listeners are registered passive — the probe never cancels a wheel event,
 * so a healthy pane scrolls exactly as it would without this installed. What it
 * does do, once a gesture is judged frozen, is drive `scrollTop` itself for the
 * rest of that gesture. That judgement is made **one frame after the first
 * wheel event**, not when the gesture ends: repairing from the gesture-end
 * timer left a frozen pane visually dead for the entire swipe and then jumped
 * it into place on release, which reads as a worse bug than the freeze.
 *
 * Returns a teardown function.
 */
export function installScrollFreezeProbe(
  el: HTMLElement,
  ctx: ProbeContext,
  sink: DiagSink,
  now: () => number = () => Date.now(),
  revive: (el: HTMLElement) => boolean = reviveScrollLayer,
): () => void {
  let gestureTimer: number | null = null;
  let startTop: number | null = null;
  let startPins = 0;
  let intent = 0;
  let lastReportAt = 0;
  /** Whether this gesture's scrolling is being driven by us. */
  let takeover = false;
  /** How far the pane had moved on its own when the takeover began. */
  let nativeMoved = 0;
  /** One pending frame check per gesture, not one per wheel event. */
  let framePending = false;
  /** Set by teardown, so a queued frame callback can't act on a dead pane. */
  let stopped = false;

  const overflowOf = () => el.scrollHeight - el.clientHeight;

  /**
   * Put the viewport where the reader's wheels have asked for.
   *
   * Absolute — `gesture start + total intent` — never `scrollTop += delta`. If
   * WebKit was merely reporting a stale `scrollTop` and later applies the same
   * wheel events itself, a relative step would land the reader twice as far as
   * they asked; recomputing from the gesture's origin is idempotent, so the
   * worst case is agreeing with the browser.
   */
  const driveToIntent = () => {
    if (startTop === null) return;
    el.scrollTop = Math.max(0, Math.min(overflowOf(), startTop + intent));
  };

  /**
   * A frame after the gesture began: did the pane move on its own?
   *
   * Runs the same verdict rules as the gesture-end report, so "frozen" means
   * one thing in this file. Anything else — it scrolled, there was nowhere to
   * go, the auto-follow pin grabbed it — is left alone: driving `scrollTop`
   * against a working pane, or against the pin, would be its own bug.
   */
  const considerTakeover = () => {
    framePending = false;
    if (stopped || takeover || startTop === null) return;

    const overflow = overflowOf();
    const moved = el.scrollTop - startTop;
    const verdict = classifyScrollAttempt({
      overflow,
      room: intent > 0 ? overflow - startTop : startTop,
      moved,
      intent,
      pinFired: ctx.pinCount() > startPins,
    });
    if (verdict !== "frozen") return;

    // Rebuild the stale scroll layer first: writing `scrollTop` is what moves
    // the reader, but leaving the layer stale means every later frame of this
    // gesture — and the next one — starts from the same dead state.
    if (!revive(el)) return;
    nativeMoved = moved;
    takeover = true;
    driveToIntent();
  };

  const finish = () => {
    gestureTimer = null;
    if (startTop === null) return;

    const beganAt = startTop;
    const beganPins = startPins;
    const gestureIntent = intent;
    const tookOver = takeover;
    const movedBeforeTakeover = nativeMoved;
    startTop = null;
    intent = 0;
    // Each gesture re-tests the pane. A latched takeover would keep overriding
    // native scrolling once it recovers, and one misjudgement would then last
    // the rest of the session.
    takeover = false;
    nativeMoved = 0;

    const scrollTop = el.scrollTop;
    const overflow = overflowOf();
    const room = gestureIntent > 0 ? overflow - beganAt : beganAt;
    const attempt: ScrollAttempt = {
      overflow,
      room,
      moved: scrollTop - beganAt,
      intent: gestureIntent,
      pinFired: ctx.pinCount() > beganPins,
    };

    // A gesture we drove is a freeze by definition — the frame check already
    // said so. Re-classifying it would read our own `scrollTop` writes as a
    // healthy `moved` and quietly drop every repaired freeze from the log,
    // leaving no evidence that the fault is still happening.
    const verdict = tookOver ? "frozen" : classifyScrollAttempt(attempt);
    if (!isFaultVerdict(verdict)) return;

    // A frozen container is the one fault this can actually repair: the scroll
    // layer is stale, so rebuild it and finish the gesture the reader made —
    // reviving alone would leave them looking at the same unmoved viewport,
    // having to scroll again. The other verdicts are our own bugs, and
    // rebuilding the layer would just paper over them.
    //
    // Repair on every frozen gesture, not just the ones that get logged: the
    // log is throttled so a stuck pane can't flood it, but each gesture the
    // reader makes deserves an attempt.
    let repair = "";
    if (tookOver) {
      // Already repaired mid-gesture. `nativeMoved` is what the pane managed on
      // its own before we stepped in, so a log full of `nativeMoved=0` says the
      // freeze is untouched underneath and only the symptom is being handled.
      repair =
        ` takeover=1 nativeMoved=${Math.round(movedBeforeTakeover)}` +
        ` landed=${Math.round(el.scrollTop)}`;
    } else if (verdict === "frozen") {
      // Reached when the frame check never ran — a backgrounded window pauses
      // rAF, so the gesture-end timer stays the backstop.
      const before = el.scrollTop;
      if (revive(el)) {
        el.scrollTop = Math.max(0, Math.min(overflow, beganAt + gestureIntent));
        const landed = el.scrollTop;
        // Says in the log whether the repair actually repaired anything,
        // rather than leaving that claim to rest on anyone's say-so.
        repair = ` revive=${landed !== before ? "recovered" : "failed"} landed=${Math.round(landed)}`;
      } else {
        repair = " revive=skipped";
      }
    }

    const at = now();
    if (at - lastReportAt < REPORT_COOLDOWN_MS) return;
    lastReportAt = at;

    const rect = el.getBoundingClientRect();
    const growth = ctx.contentGrowth();
    sink(
      [
        `scroll-freeze verdict=${verdict}`,
        `session=${ctx.sessionId() ?? "?"}`,
        `status=${ctx.status() ?? "?"}`,
        `msgs=${ctx.messageCount()}`,
        `following=${ctx.isFollowing()}`,
        `intent=${Math.round(gestureIntent)}`,
        `moved=${Math.round(attempt.moved)}`,
        `room=${Math.round(room)}`,
        `scrollTop=${Math.round(scrollTop)}`,
        `scrollHeight=${Math.round(el.scrollHeight)}`,
        `clientHeight=${Math.round(el.clientHeight)}`,
        // A box whose laid-out height disagrees with its client height, or that
        // extends past the window, means the freeze is a layout fault rather
        // than a scrolling one — worth seeing side by side with the verdict.
        `rectHeight=${Math.round(rect.height)}`,
        `rectTop=${Math.round(rect.top)}`,
        `rectBottom=${Math.round(rect.bottom)}`,
        `winHeight=${Math.round(window.innerHeight)}`,
        `pins=${ctx.pinCount()}`,
        // "none" rather than a number: an idle transcript that settled long ago
        // still freezes, so "never grew" has to stay distinguishable from
        // "grew 0px just now" or the hypothesis can't be falsified.
        `sinceGrow=${growth ? Math.round(growth.sinceMs) : "none"}`,
        `growBy=${growth ? Math.round(growth.byPx) : 0}`,
      ].join(" ") + repair,
    );
  };

  const onWheel = (ev: WheelEvent) => {
    // Not this pane's gesture — a card inside it is taking the scroll. Bailing
    // before the gesture even opens also keeps a takeover from latching onto
    // one, which would drive the pane for every wheel event that follows.
    if (nestedScrollerWillConsume(el, ev)) return;
    if (startTop === null) {
      startTop = el.scrollTop;
      startPins = ctx.pinCount();
      intent = 0;
      takeover = false;
      nativeMoved = 0;
    }
    intent += ev.deltaY;

    if (takeover) {
      // Straight through, no frame wait: this is what makes a repaired pane
      // track the wheel in real time instead of catching up on release.
      driveToIntent();
    } else if (!framePending) {
      framePending = true;
      window.requestAnimationFrame(considerTakeover);
    }

    if (gestureTimer !== null) window.clearTimeout(gestureTimer);
    gestureTimer = window.setTimeout(finish, GESTURE_IDLE_MS);
  };

  el.addEventListener("wheel", onWheel, { passive: true });
  return () => {
    stopped = true;
    el.removeEventListener("wheel", onWheel);
    if (gestureTimer !== null) window.clearTimeout(gestureTimer);
  };
}
