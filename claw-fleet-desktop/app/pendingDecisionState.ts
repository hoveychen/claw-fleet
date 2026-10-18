import type { PendingDecision } from "./types";

/**
 * Whether a session has a card waiting on the user, and whether that card has
 * already timed out (parked).
 *
 * Cards render only inside their task's conversation (the decision dock), so
 * a card raised on a session you are not reading has no surface of its own;
 * views that draw sessions (the lobster fleet) wear this state instead.
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
