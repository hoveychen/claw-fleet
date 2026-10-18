// "Just woken" latch for the task list, shared by the desktop home and the
// phone so both hold a waking task in the same section.
//
// Why it exists: a session's status is derived from its transcript, and a
// session being woken up has not written anything yet. When the boss answers
// a card, a `fleet watch` fires, or a parked card's answer resumes the session,
// the thing that kept the task out of "review" goes away at once — the card
// leaves the pending list, the watch is deleted — while the transcript still
// ends on the previous turn's `end_turn`, so the status reads `waitingInput`
// until the resumed turn writes its first line. Measured on 2026-10-19 in an
// isolated `fleet webui`: a watch-woken coordinator sat at `waitingInput` with
// its new process already alive for ~2s before turning `thinking`, and the
// five-state derivation filed it under 待验收 in between.
//
// The latch arms only on an observed transition — the session was parked on
// the boss or a watch and no longer is, or its process was dead and now lives
// — and holds until the transcript moves (any new `lastActivityMs`) or
// `WAKE_HOLD_MS` passes. Nothing is inferred from a single snapshot, so a
// session whose snapshot merely *reads* alive after its turn ended is left
// alone.

/** Longest a woken session is held as moving without writing anything. A
 *  resume that has not produced a transcript line by then has most likely
 *  failed to start, and the honest state is whatever the scan says. */
export const WAKE_HOLD_MS = 20_000;

/** Entries unseen this long are dropped, only to bound the map. */
const ENTRY_TTL_MS = 3_600_000;

type Entry = {
  parked: boolean;
  alive: boolean;
  /** `lastActivityMs` when the latch armed; null while not armed. */
  armedActivity: number | null;
  /** When an armed latch gives up. */
  until: number;
  seenAt: number;
};

export type WakeLatchState = Map<string, Entry>;

export function createWakeLatch(): WakeLatchState {
  return new Map();
}

export function resetWakeLatch(state: WakeLatchState): void {
  state.clear();
}

export type WakeObservation = {
  /** Waiting on the boss or on a timer: a card from this session is pending,
   *  or it is parked on a `fleet watch`. */
  parked: boolean;
  alive: boolean;
  lastActivityMs: number;
  now: number;
};

/** Whether the session is in the gap between being woken and its first
 *  transcript write. Idempotent for a repeated observation, so every view
 *  that builds rows may call it for the same session. */
export function stickyWaking(state: WakeLatchState, id: string, obs: WakeObservation): boolean {
  const { parked, alive, lastActivityMs, now } = obs;
  const prev = state.get(id);
  const woke = !!prev && ((prev.parked && !parked) || (!prev.alive && alive));

  let armedActivity: number | null = prev?.armedActivity ?? null;
  let until = prev?.until ?? 0;
  if (parked) {
    armedActivity = null;
  } else if (woke) {
    armedActivity = lastActivityMs;
    until = now + WAKE_HOLD_MS;
  } else if (armedActivity !== null && (lastActivityMs !== armedActivity || now >= until)) {
    armedActivity = null;
  }

  state.set(id, { parked, alive, armedActivity, until, seenAt: now });
  if (state.size > 500) {
    for (const [k, e] of state) if (now - e.seenAt > ENTRY_TTL_MS) state.delete(k);
  }
  return armedActivity !== null;
}

/** `parked` as both clients read it off a session: a pending card, or a
 *  `fleet watch` (the `watching` status, or watches still registered). */
export function isParkedForWake(
  s: { status: string; watches?: unknown[] | null },
  hasPendingDecision: boolean,
): boolean {
  return hasPendingDecision || s.status === "watching" || (s.watches?.length ?? 0) > 0;
}
