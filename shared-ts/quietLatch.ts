// A shared "quiet-alive hysteresis" for desktop and mobile: prevents the
// status dot from flashing between green and gray.
//
// This directory is the only cross-frontend-package code-sharing point in the
// repo and contains only zero-dependency pure functions (see procShell.ts).
// The reason for sharing here is the same: desktop `rowBarColor` and mobile
// `statusTone` are parallel implementations of the same semantic, both suffer
// from flickering, and maintaining separate logic would cause drift.
//
// Why hysteresis is needed: core's `determine_status` only looks at the last
// few transcript records *plus their age*. Each active branch has a hard
// window (`stop_reason=tool_use` keeps 60s Executing, user messages keep 120s
// Thinking, falls back to Idle after 30s). So a session stuck on *a long
// tool call* (running build, waiting for background tasks) writes a transcript
// line every few minutes: each write pushes status back to active (green), the
// window decays to idle (gray), flashing several times per minute.
//
// Trade-off (approved): gray, once lit, stays sticky; the downside is that
// "truly stuck" signals become less responsive. Recovery doesn't rely on
// timeout but on *writes clustering*: only when two adjacent writes are
// ≤ DENSE_WRITE_MS apart do we conclude the session is truly active again,
// and switch back to green. A single sparse write only updates the baseline,
// doesn't unlock.

/** Threshold interval for detecting "writes clustering". Set to 30s—exactly
 *  the window for core's fallback to Idle: two writes closer than this would
 *  never decay to quiet anyway, so only real sustained activity triggers this. */
export const DENSE_WRITE_MS = 30_000;

/** Entries not observed beyond this TTL are pruned, purely to prevent the Map
 *  from growing unbounded with every scanned session. Not "gray times out back
 *  to green"—that's the jitter we're trying to avoid. */
const ENTRY_TTL_MS = 3_600_000;

type Entry = {
  /** Most recent time observed as raw quiet (for TTL pruning only). */
  quietAt: number;
  /** Latest observed transcript activity timestamp; measures gap to next write. */
  lastActivityMs: number;
  /** Most recent observation time (for TTL pruning). */
  seenAt: number;
};

export type QuietLatchState = Map<string, Entry>;

export function createQuietLatch(): QuietLatchState {
  return new Map();
}

export type QuietObservation = {
  /** Whether the process is still alive. Once gone, hysteresis immediately
   *  invalidates; otherwise a truly ended session would stick at gray, and if
   *  its id is reused on resume, the new session starts dark. */
  alive: boolean;
  /** Raw judgment (before hysteresis): process is alive but scan-computed
   *  status has decayed to "ended". */
  rawQuiet: boolean;
  /** Most recent activity timestamp of this session's transcript
   *  (`SessionInfo.lastActivityMs`). */
  lastActivityMs: number;
  now: number;
};

/** Quiet-alive judgment with hysteresis. Idempotent: can be called every render
 *  for the same session; re-observing the same `lastActivityMs` is not treated
 *  as a second write. */
export function stickyQuiet(
  state: QuietLatchState,
  id: string,
  { alive, rawQuiet, lastActivityMs, now }: QuietObservation,
): boolean {
  if (!alive) {
    state.delete(id);
    return false;
  }
  const prev = state.get(id);
  if (rawQuiet) {
    state.set(id, {
      quietAt: now,
      // Baseline on entering quiet is "known latest activity time"—scan-side
      // lastActivityMs can regress (snapshot merge, delta fill), baseline only
      // advances; one regression would fake a dense interval.
      lastActivityMs: Math.max(lastActivityMs, prev?.lastActivityMs ?? lastActivityMs),
      seenAt: now,
    });
    return true;
  }
  if (!prev) return false;
  prune(state, now);
  const gap = lastActivityMs - prev.lastActivityMs;
  if (gap > 0 && gap <= DENSE_WRITE_MS) {
    // Two adjacent writes are close: session truly producing again, unlock to green.
    state.delete(id);
    return false;
  }
  if (gap > 0) {
    // Single sparse write: don't unlock, just advance baseline so *next* write's gap measures from here.
    state.set(id, { ...prev, lastActivityMs, seenAt: now });
  } else {
    state.set(id, { ...prev, seenAt: now });
  }
  return true;
}

function prune(state: QuietLatchState, now: number): void {
  if (state.size < 256) return;
  for (const [k, v] of state) {
    if (now - v.seenAt > ENTRY_TTL_MS) state.delete(k);
  }
}

/** For tests: clear hysteresis state so test cases don't bleed into each other. */
export function resetQuietLatch(state: QuietLatchState): void {
  state.clear();
}
