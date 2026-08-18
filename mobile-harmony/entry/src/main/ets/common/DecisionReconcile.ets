import { PendingDecision } from '../models/Types';

/// Native port of mobile-web/src/decisionReconcile.ts — the reconcile that keeps
/// the pending-decision list truthful against the authoritative pending_snapshot
/// without letting a just-answered card flicker back before the desktop confirms.
///
/// The phone answers a card fire-and-forget (`RelayClient.answer` reports only
/// whether the frame reached the socket, never whether it took effect). So the
/// UI removes an answered card optimistically AND records its id + timestamp in
/// `answeredAt`. On every snapshot pull this reconcile:
///   - keeps the snapshot authoritative for what's still pending, but
///   - hides cards whose local answer is still in flight (the snapshot lags the
///     just-sent answer on a slow link — without this the card flickers back), and
///   - lets a card *return* once its answer is presumed lost: if the desktop
///     still lists it past ANSWER_GRACE_MS, the answer frame was dropped, so the
///     card comes back for the user to re-answer.
/// The happy path clears far sooner — the moment a snapshot no longer lists the
/// card (positive confirmation the desktop processed it).

/// How long to keep suppressing a card answered on THIS device but not yet
/// confirmed processed by the desktop. Fallback ceiling for a lost answer frame.
export const ANSWER_GRACE_MS = 30000;

export interface ReconcileInput {
  /// The list currently rendered (used to preserve each card's arrival stamp so
  /// a refresh doesn't reshuffle the queue).
  prev: PendingDecision[];
  /// The authoritative snapshot pulled from the desktop.
  fresh: PendingDecision[];
  /// id → timestamp of a local answer still awaiting desktop confirmation.
  answeredAt: Map<string, number>;
  /// Current wall clock (injected for testability).
  now: number;
}

export interface ReconcileResult {
  decisions: PendingDecision[];
  /// Pruned copy of `answeredAt` (confirmed-landed and grace-expired ids removed).
  answeredAt: Map<string, number>;
}

export function reconcileDecisions(input: ReconcileInput): ReconcileResult {
  const freshIds = new Set<string>();
  for (const d of input.fresh) {
    freshIds.add(d.id);
  }
  // Prune the in-flight-answer set: an id the desktop dropped from the snapshot
  // is confirmed processed (clear it); an id still listed past the grace window
  // is a presumed-lost answer (clear it so the card can return).
  const nextAnswered = new Map<string, number>();
  input.answeredAt.forEach((at: number, id: string) => {
    if (freshIds.has(id) && input.now - at < ANSWER_GRACE_MS) {
      nextAnswered.set(id, at);
    }
  });
  const seen = new Map<string, number>();
  for (const d of input.prev) {
    seen.set(d.id, d.arrivedAt);
  }
  const decisions: PendingDecision[] = [];
  for (const d of input.fresh) {
    // Hide cards whose local answer is still in flight.
    if (nextAnswered.has(d.id)) {
      continue;
    }
    decisions.push({
      kind: d.kind,
      id: d.id,
      request: d.request,
      arrivedAt: seen.get(d.id) ?? d.arrivedAt,
    });
  }
  return { decisions, answeredAt: nextAnswered };
}
