import type { PendingDecision } from "./types";

/** How long to keep suppressing a card the user answered on THIS device but
 *  whose answer hasn't been confirmed processed by the desktop yet. The happy
 *  path clears far sooner — the moment a snapshot no longer lists the card
 *  (positive confirmation). This timer is only the fallback for a genuinely
 *  lost answer frame (fire-and-forget send dropped on a flaky link): after it
 *  elapses the card is allowed back so Boss can re-answer. */
export const ANSWER_GRACE_MS = 30_000;

/** Reconcile the authoritative `pending_snapshot` against the locally-held
 *  card list, honouring cards the user just answered on THIS device whose
 *  answer is still in flight. */
export interface ReconcileInput {
  /** The list currently rendered (used to preserve each card's arrival stamp
   *  so a refresh doesn't reshuffle the queue). */
  prev: PendingDecision[];
  /** The authoritative snapshot pulled from the desktop. */
  fresh: PendingDecision[];
  /** id → timestamp of a local answer still awaiting desktop confirmation. */
  answeredAt: Map<string, number>;
  /** Current wall clock (injected for testability). */
  now: number;
}

export interface ReconcileResult {
  decisions: PendingDecision[];
  /** Pruned copy of `answeredAt` (confirmed-landed and grace-expired ids removed). */
  answeredAt: Map<string, number>;
}

export function reconcileDecisions({
  prev,
  fresh,
  answeredAt,
  now,
}: ReconcileInput): ReconcileResult {
  const freshIds = new Set(fresh.map((d) => d.id));
  // Prune the in-flight-answer set: an id the desktop has dropped from the
  // snapshot is confirmed processed (clear it); an id still listed past the
  // grace window is a presumed-lost answer (clear it so the card can return).
  const nextAnswered = new Map<string, number>();
  for (const [id, at] of answeredAt) {
    if (freshIds.has(id) && now - at < ANSWER_GRACE_MS) nextAnswered.set(id, at);
  }
  const seen = new Map(prev.map((d) => [d.id, d.arrivedAt]));
  const decisions = fresh
    // Hide cards whose local answer is still in flight — the snapshot lags the
    // just-sent answer on a slow link, and without this the card flickers back.
    .filter((d) => !nextAnswered.has(d.id))
    .map((d) => ({ ...d, arrivedAt: seen.get(d.id) ?? d.arrivedAt }));
  return { decisions, answeredAt: nextAnswered };
}
