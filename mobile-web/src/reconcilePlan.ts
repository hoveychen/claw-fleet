/** Cadence for the foreground pending-decision reconcile loop.
 *
 *  Two safety nets recover a `decision_created` / `decision_resolved` broadcast
 *  the phone missed (both are best-effort agent→client frames — no ack, no
 *  queue, no retry): the one-shot `refreshPending` on (re)connect / foreground,
 *  and this periodic reconcile. On a weak link the one-shot pull can time out
 *  and is swallowed, so the periodic loop is the only thing that eventually
 *  re-pulls the authoritative snapshot. It therefore must keep ticking even
 *  when zero cards are shown — otherwise a card missed while the socket was
 *  down (the only way to miss a frame over TCP) never gets pulled and strands
 *  forever. See decisionReconcile.ts for the merge, App.tsx for the wiring. */
export const DECISION_RECONCILE_ACTIVE_MS = 3_000;
export const DECISION_RECONCILE_IDLE_MS = 15_000;
/** Probe cadence while the desktop looks offline. */
export const DECISION_RECONCILE_OFFLINE_MS = 30_000;

export interface ReconcilePlan {
  /** Whether the reconcile loop should be scheduled at all. */
  poll: boolean;
  /** Delay between ticks when it is. */
  intervalMs: number;
}

/** Decide whether — and how often — to poll `pending_snapshot` as a fallback
 *  for missed decision broadcasts. Poll whenever the desktop is online: fast
 *  while cards are open (so an answered-elsewhere card clears promptly), slower
 *  when idle (just catching up on any card missed during a weak-link drop). */
export function reconcilePlan(agentOnline: boolean, hasPendingDecisions: boolean): ReconcilePlan {
  if (!agentOnline) return { poll: false, intervalMs: DECISION_RECONCILE_IDLE_MS };
  return {
    poll: true,
    intervalMs: hasPendingDecisions ? DECISION_RECONCILE_ACTIVE_MS : DECISION_RECONCILE_IDLE_MS,
  };
}
