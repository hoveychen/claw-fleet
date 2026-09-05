import { describe, expect, it } from "vitest";
import {
  DECISION_RECONCILE_ACTIVE_MS,
  DECISION_RECONCILE_IDLE_MS,
  DECISION_RECONCILE_OFFLINE_MS,
  reconcilePlan,
} from "./reconcilePlan";

describe("reconcilePlan", () => {
  // Boss's weak-network bug: on the same-origin webui the SSE stream is the
  // only thing that flips `agentOnline`, and requests ride a separate plain
  // `fetch` that keeps working while the stream is down. Stopping the poll on
  // `agentOnline === false` therefore removed the one channel that could still
  // recover a card — the stream was dead AND nothing polled, so only a page
  // reload brought cards back. Probe slowly instead of not at all.
  it("still probes slowly while the desktop looks offline", () => {
    expect(reconcilePlan(false, false)).toEqual({
      poll: true,
      intervalMs: DECISION_RECONCILE_OFFLINE_MS,
    });
    expect(reconcilePlan(false, true)).toEqual({
      poll: true,
      intervalMs: DECISION_RECONCILE_OFFLINE_MS,
    });
  });

  it("polls fast while cards are open", () => {
    const plan = reconcilePlan(true, true);
    expect(plan.poll).toBe(true);
    expect(plan.intervalMs).toBe(DECISION_RECONCILE_ACTIVE_MS);
  });

  // The weak-network regression: a card missed while the socket was down never
  // gets added locally, so the loop must keep polling with zero cards shown to
  // eventually re-pull the snapshot. The old logic returned poll:false here,
  // which is exactly why such a card stranded forever.
  it("keeps polling (slowly) when idle so a missed card is recovered", () => {
    const plan = reconcilePlan(true, false);
    expect(plan.poll).toBe(true);
    expect(plan.intervalMs).toBe(DECISION_RECONCILE_IDLE_MS);
  });
});
