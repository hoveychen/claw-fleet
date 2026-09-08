import { describe, it, expect } from "vitest";
import { flattenPending, reconcilePlan } from "./decisionReconcile";
import type { PendingDecision, PendingDecisions } from "./types";

function card(id: string, parked = false): PendingDecision {
  return {
    kind: "fleet-ask",
    id,
    request: { id, sessionId: "s1", parked },
    answers: {},
    arrivedAt: 0,
  } as unknown as PendingDecision;
}

function snapshot(over: Partial<Record<keyof PendingDecisions, unknown[]>> = {}): PendingDecisions {
  return {
    guard: [],
    elicitation: [],
    fleetAsk: [],
    a2uiRender: [],
    planApproval: [],
    permissionPrompt: [],
    ...over,
  } as unknown as PendingDecisions;
}

describe("flattenPending", () => {
  it("collects ids from every one of the six channels", () => {
    const flat = flattenPending(
      snapshot({
        guard: [{ id: "g1" }],
        elicitation: [{ id: "e1" }],
        fleetAsk: [{ id: "f1" }],
        a2uiRender: [{ id: "a1" }],
        planApproval: [{ id: "p1" }],
        permissionPrompt: [{ id: "m1" }],
      }),
    );
    expect([...flat.keys()].sort()).toEqual(["a1", "e1", "f1", "g1", "m1", "p1"]);
  });

  // An older `fleet serve` omits `permissionPrompt`; the buckets are read with
  // `?.forEach` for that reason and a missing one must not throw.
  it("tolerates a snapshot with buckets missing", () => {
    const flat = flattenPending({ fleetAsk: [{ id: "f1" }] } as unknown as PendingDecisions);
    expect([...flat.keys()]).toEqual(["f1"]);
  });
});

describe("reconcilePlan", () => {
  it("plans nothing when the store already agrees with the backend", () => {
    const plan = reconcilePlan(
      [card("f1")],
      new Set(["f1"]),
      flattenPending(snapshot({ fleetAsk: [{ id: "f1" }] })),
    );
    expect(plan).toEqual({ drop: [], park: [] });
  });

  // A lost `*-dismissed` emit: the backend cleaned the request up, the store
  // never heard, and the user gets "no pending request" on answering.
  it("drops a card the backend no longer has pending", () => {
    const plan = reconcilePlan([card("f1")], new Set(["f1"]), flattenPending(snapshot()));
    expect(plan.drop).toEqual(["f1"]);
  });

  // The race this guard exists for: a card that arrived over the live channel
  // *after* the snapshot was requested is legitimately absent from the answer,
  // because the backend read its directory before that card existed. Dropping
  // it would delete a brand-new card the user is looking at.
  it("leaves a card that arrived while the snapshot was in flight", () => {
    const plan = reconcilePlan(
      [card("f1"), card("f2")],
      new Set(["f1"]),
      flattenPending(snapshot({ fleetAsk: [{ id: "f1" }] })),
    );
    expect(plan.drop).toEqual([]);
  });

  // A lost `decision-parked` emit: the card timed out and the turn was
  // interrupted, but it keeps showing a running countdown.
  it("parks a card the backend reports as parked", () => {
    const plan = reconcilePlan(
      [card("f1")],
      new Set(["f1"]),
      flattenPending(snapshot({ fleetAsk: [{ id: "f1", parked: true }] })),
    );
    expect(plan).toEqual({ drop: [], park: ["f1"] });
  });

  it("does not re-park a card that is already parked", () => {
    const plan = reconcilePlan(
      [card("f1", true)],
      new Set(["f1"]),
      flattenPending(snapshot({ fleetAsk: [{ id: "f1", parked: true }] })),
    );
    expect(plan.park).toEqual([]);
  });

  // A parked card stays in the backend's pending set — it is waiting for an
  // answer that will resume the interrupted session — so it must never be
  // pruned as stale.
  it("never drops a parked card", () => {
    const plan = reconcilePlan(
      [card("f1", true)],
      new Set(["f1"]),
      flattenPending(snapshot({ fleetAsk: [{ id: "f1", parked: true }] })),
    );
    expect(plan.drop).toEqual([]);
  });
});
