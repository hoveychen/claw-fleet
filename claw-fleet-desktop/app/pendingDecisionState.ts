import type { PendingDecision } from "./types";

/**
 * Whether a session has a card waiting on the user, and whether that card has
 * already timed out (parked).
 *
 * Needed because simplified mode has no always-on `DecisionPanel`: the card is
 * only rendered inside the session detail, so a card raised on a task you are
 * not currently reading has no surface of its own. The task row wears this, so
 * a *parked* card is visible without opening every task in turn — and the
 * header pill (`SimpleNavigation`) uses the same store to say that something is
 * waiting at all, whichever page you are on.
 *
 * `parked` wins over `pending` when a session somehow has both — the timed-out
 * one is the one that stopped a turn and is holding the session.
 */
export type PendingDecisionState = "none" | "pending" | "parked";

/**
 * The card that has been waiting longest, or `null` when nothing is pending.
 *
 * Which one the global pill in simplified mode's header jumps to. Oldest first
 * because that is the session that has been blocked longest — and for a parked
 * card, the one whose turn was already interrupted.
 *
 * `arrivedAt` is when *this client* first saw the card, not when the agent
 * raised it, so a restart re-stamps them all. Good enough for picking a
 * destination: the ordering is still the order they came back in.
 */
export function oldestPendingDecision(
  decisions: PendingDecision[],
): PendingDecision | null {
  let oldest: PendingDecision | null = null;
  for (const d of decisions) {
    if (!oldest || d.arrivedAt < oldest.arrivedAt) oldest = d;
  }
  return oldest;
}

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
