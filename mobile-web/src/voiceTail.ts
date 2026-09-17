// What to do with that **still-unfinalized real-time transcription** when recording stops.
//
// Background: the recognition engine splits one utterance into segments, each segment
// first appears as a partial real-time echo, then gets finalized. Only final makes it
// into the input box; partial is just floating text on screen. When the user hits stop,
// the last segment often is still in partial form — whether it survives depends entirely
// on the engine giving us a final **after** shutting down.
//
// But that's unreliable: in HarmonyOS shells, the `end` (engine shutdown) event kills the
// callback hook immediately. If it fires before the final, that text has no outlet. Say
// a short phrase in the decision card's "Other" and it's often a partial from start to
// end — hit stop and the text vanishes entirely, as if you never typed. The "certain
// probability" is because it depends on which of those two native callbacks arrives first.
//
// So we don't bet on the engine giving us the final anymore: after stop, we leave a
// grace window. If the final arrives, it takes over (use its text, don't duplicate). If
// the timer expires, we commit the last text the user saw ourselves. **We'd rather
// supplement a slightly rough transcript than let what the user said vanish into thin
// air** — the former they can see and edit, the latter they don't even know happened.

/**
 * How long to wait after stop for the engine to deliver a finalized segment.
 *
 * **Shorter** than useVoiceRecorder's FINALIZE_MS (1200ms, the "stop and send" finalization
 * wait time): this way the supplemented text arrives in time for that send, so we don't
 * get "message just sent is missing the last sentence".
 */
export const TAIL_GRACE_MS = 900;

export interface TailOptions {
  /** Override grace duration (for tests). */
  graceMs?: number;
  /**
   * Waiting is done (supplemented / finalization arrived / cancelled).
   *
   * The UI uses this to clear the "still-unfinalized" text from the screen. Waiting for
   * supplement takes nearly a second; if we clear the text first and bring it back later,
   * the user still sees "text disappeared", just briefly. So the UI waits for settle
   * before clearing.
   */
  onSettle?: () => void;
}

export interface TailGuard {
  /** Received a real-time partial transcription. */
  partial(text: string): void;
  /** Received a finalized segment — it takes over, any pending supplement becomes void. */
  final(): void;
  /**
   * User hit stop: start waiting for finalization, commit pending if timer expires.
   *
   * @returns Whether we actually entered wait state (only if there's something to wait
   *          for). The UI uses this to decide whether to hold a frame.
   */
  stop(): boolean;
  /** User hit cancel / discard this recognition: don't supplement anything. */
  cancel(): void;
  /** Component unmounted: clean up the timer, don't write to an input that's gone. */
  dispose(): void;
}

/**
 * @param commit Commit a text segment. Its semantics are identical to finalization
 *               (callers can't and shouldn't distinguish the two).
 */
export function createTailGuard(
  commit: (text: string) => void,
  opts: TailOptions = {},
): TailGuard {
  const graceMs = opts.graceMs ?? TAIL_GRACE_MS;
  /** The current segment's real-time partial; cleared once finalized (that segment has
   *  an outlet now). */
  let pending = "";
  let timer: ReturnType<typeof setTimeout> | null = null;
  let dead = false;

  /** The waiting period after stop, before we have a result. */
  let awaiting = false;

  const disarm = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  /** End the wait and notify the UI. No-op if not waiting — don't report spuriously. */
  const settle = () => {
    disarm();
    if (!awaiting) return;
    awaiting = false;
    opts.onSettle?.();
  };

  return {
    partial(text) {
      if (dead) return;
      pending = text;
      // Even after stop, longer echoes may still arrive (web-speech's stop is async).
      // Don't reset the timer: commit timing follows "when stop was pressed", content
      // follows "the last thing the user saw".
    },
    final() {
      if (dead) return;
      pending = "";
      settle();
    },
    stop() {
      if (dead) return false;
      disarm();
      if (!pending) return false;
      awaiting = true;
      timer = setTimeout(() => {
        timer = null;
        const text = pending;
        pending = "";
        awaiting = false;
        if (text) commit(text);
        opts.onSettle?.();
      }, graceMs);
      return true;
    },
    cancel() {
      if (dead) return;
      pending = "";
      settle();
    },
    dispose() {
      dead = true;
      pending = "";
      awaiting = false;
      disarm();
    },
  };
}
