import { describe, expect, it } from "vitest";
import {
  DECISION_RECONCILE_ACTIVE_MS,
  DECISION_RECONCILE_IDLE_MS,
  reconcilePlan,
} from "./reconcilePlan";

describe("reconcilePlan", () => {
  it("does not poll while the desktop is offline", () => {
    expect(reconcilePlan(false, false).poll).toBe(false);
    expect(reconcilePlan(false, true).poll).toBe(false);
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
