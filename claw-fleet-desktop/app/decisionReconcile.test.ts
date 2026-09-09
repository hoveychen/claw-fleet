import { describe, it, expect, beforeEach } from "vitest";
import {
  REMOVED_GRACE_MS,
  announcementFor,
  clearRemovedLocally,
  flattenPending,
  noteRemovedLocally,
  reconcilePlan,
  suppressedIds,
} from "./decisionReconcile";
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

/**
 * The regression the grace window exists for: the reconcile poll is faster
 * than the answer round trip. Answering removes the card and posts the
 * response in the background; the request file survives on disk until the
 * blocked producer notices it on its own 200ms poll. A reconcile in that gap
 * sees the card as still pending, and re-adding it would put an
 * already-answered question back in front of the user.
 */
describe("locally removed cards", () => {
  beforeEach(() => clearRemovedLocally());

  it("suppresses a card this client just removed", () => {
    const now = 1_000_000;
    noteRemovedLocally("f1", now);
    expect(suppressedIds(now)).toEqual(new Set(["f1"]));
  });

  it("stops suppressing once the grace has passed, so a genuinely lost answer resurfaces", () => {
    const now = 1_000_000;
    noteRemovedLocally("f1", now);
    expect(suppressedIds(now + REMOVED_GRACE_MS)).toEqual(new Set(["f1"]));
    expect(suppressedIds(now + REMOVED_GRACE_MS + 1)).toEqual(new Set());
  });

  it("does not suppress cards it never saw removed", () => {
    expect(suppressedIds(1_000_000)).toEqual(new Set());
  });

  // Suppression is about *adding*; a suppressed id is not in the store, so
  // there is nothing for the plan to drop or park either way.
  it("does not interfere with the drop/park plan", () => {
    const now = 1_000_000;
    noteRemovedLocally("f1", now);
    const plan = reconcilePlan(
      [card("f2")],
      new Set(["f2"]),
      flattenPending(snapshot({ fleetAsk: [{ id: "f1" }] })),
    );
    expect(plan.drop).toEqual(["f2"]);
  });
});

describe("announcementFor", () => {
  it("announces a card the poll is the first to see on a live page", () => {
    expect(announcementFor("interval", false, false)).toBe("announce");
    expect(announcementFor("visible", undefined, false)).toBe("announce");
  });

  /**
   * The regression this function exists for: the mount pass must be silent
   * *and* remembered. When it only did the first half, the 10s tick behind it
   * re-announced every card that predated the page — a chime for a card that
   * had been sitting there for half an hour, on every page load and every
   * remount, with nothing new on screen to explain it.
   */
  it("records a card seen on mount instead of leaving it to be re-announced", () => {
    expect(announcementFor("mount", false, false)).toBe("record");
    // Recorded means the next tick sees it as already announced → silence.
    expect(announcementFor("interval", false, true)).toBe("skip");
  });

  it("never announces or records a parked card", () => {
    expect(announcementFor("interval", true, false)).toBe("skip");
    expect(announcementFor("mount", true, false)).toBe("skip");
  });

  it("stays quiet about a card it already announced", () => {
    expect(announcementFor("interval", false, true)).toBe("skip");
    expect(announcementFor("mount", false, true)).toBe("skip");
  });
});
