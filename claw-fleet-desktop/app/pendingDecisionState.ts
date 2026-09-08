import type { PendingDecision } from "./types";

/**
 * Whether a session has a card waiting on the user, and whether that card has
 * already timed out (parked).
 *
 * Needed because simplified mode has no always-on `DecisionPanel`: the card is
 * only rendered inside the session detail, so a card raised on a task you are
 * not currently reading has no surface at all. The task row wears this instead,
 * which is also the only place a *parked* card is visible without opening
 * every task in turn.
 *
 * `parked` wins over `pending` when a session somehow has both — the timed-out
 * one is the one that stopped a turn and is holding the session.
 */
export type PendingDecisionState = "none" | "pending" | "parked";

export function pendingDecisionState(
  decisions: PendingDecision[],
  sessionId: string,
): PendingDecisionState {
  let state: PendingDecisionState = "none";
  for (const d of decisions) {
    const req = d.request as { sessionId?: string; parked?: boolean };
    if (req.sessionId !== sessionId) continue;
    if (req.parked === true) return "parked";
    state = "pending";
  }
  return state;
}
