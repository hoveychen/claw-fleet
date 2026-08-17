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

/** Verdicts worth writing to the log. The rest are normal scrolling. */
export function isFaultVerdict(v: FreezeVerdict): boolean {
  return v === "no-overflow" || v === "yanked" || v === "frozen";
}

/** Live state the probe reads at fault time, supplied by `SessionDetail`. */
export interface ProbeContext {
  sessionId: () => string | null;
  status: () => string | null;
  messageCount: () => number;
  isFollowing: () => boolean;
  /** Total auto-follow pin writes since mount. */
  pinCount: () => number;
}

/** Writes one line to the host debug log. Injected so tests stay off Tauri. */
export type DiagSink = (line: string) => void;

/** Gesture is considered over once this long passes with no wheel event. */
const GESTURE_IDLE_MS = 140;
/** At most one report per this window, so a stuck pane can't flood the log. */
const REPORT_COOLDOWN_MS = 30_000;

/**
 * Watch `el` for scroll gestures that don't land, reporting through `sink`.
 *
 * Listeners are passive: the probe must never change how scrolling behaves,
 * or it becomes a suspect in the fault it is meant to diagnose.
 *
 * Returns a teardown function.
 */
export function installScrollFreezeProbe(
  el: HTMLElement,
  ctx: ProbeContext,
  sink: DiagSink,
  now: () => number = () => Date.now(),
): () => void {
  let gestureTimer: number | null = null;
  let startTop: number | null = null;
  let startPins = 0;
  let intent = 0;
  let lastReportAt = 0;

  const finish = () => {
    gestureTimer = null;
    if (startTop === null) return;

    const beganAt = startTop;
    const beganPins = startPins;
    const gestureIntent = intent;
    startTop = null;
    intent = 0;

    const scrollTop = el.scrollTop;
    const overflow = el.scrollHeight - el.clientHeight;
    const room = gestureIntent > 0 ? overflow - beganAt : beganAt;
    const attempt: ScrollAttempt = {
      overflow,
      room,
      moved: scrollTop - beganAt,
      intent: gestureIntent,
      pinFired: ctx.pinCount() > beganPins,
    };

    const verdict = classifyScrollAttempt(attempt);
    if (!isFaultVerdict(verdict)) return;

    const at = now();
    if (at - lastReportAt < REPORT_COOLDOWN_MS) return;
    lastReportAt = at;

    const rect = el.getBoundingClientRect();
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
      ].join(" "),
    );
  };

  const onWheel = (ev: WheelEvent) => {
    if (startTop === null) {
      startTop = el.scrollTop;
      startPins = ctx.pinCount();
      intent = 0;
    }
    intent += ev.deltaY;
    if (gestureTimer !== null) window.clearTimeout(gestureTimer);
    gestureTimer = window.setTimeout(finish, GESTURE_IDLE_MS);
  };

  el.addEventListener("wheel", onWheel, { passive: true });
  return () => {
    el.removeEventListener("wheel", onWheel);
    if (gestureTimer !== null) window.clearTimeout(gestureTimer);
  };
}
