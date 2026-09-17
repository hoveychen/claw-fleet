import { describe, expect, it } from "vitest";
import {
  reconcileDecisions,
  agentKeyOf,
  ANSWER_GRACE_MS,
  EMPTY_SNAPSHOT_COOLDOWN_MS,
} from "./decisionReconcile";
import type { PendingDecision } from "./types";

// Create a minimal decision card; kind/request content doesn't affect reconciliation,
// only id/arrivedAt matter.

function card(id: string, arrivedAt = 1_000): PendingDecision {
  return {
    kind: "guard",
    id,
    request: { id, command: "echo", sessionId: "s1" } as unknown as PendingDecision["request"],
    arrivedAt,
  };
}

describe("reconcileDecisions", () => {
  it("Normal snapshot: replace with snapshot content, preserve old card's arrivedAt without reordering", () => {
    const prev = [card("a", 100), card("b", 200)];
    const fresh = [card("b", 999), card("c", 300)];
    const { decisions } = reconcileDecisions({
      prev,
      fresh,
      answeredAt: new Map(),
      now: 10_000,
    });
    expect(decisions.map((d) => d.id)).toEqual(["b", "c"]);
    // b is an existing card: reuse its old arrivedAt(200), not the 999 from snapshot.

    expect(decisions.find((d) => d.id === "b")!.arrivedAt).toBe(200);
  });

  // This reproduces a bug the user reported: with poor network, a card just answered on
  // this device might still be in the snapshot when the desktop hasn't processed the reply
  // yet — we can't let it pop up again.
  it("Just answered locally, reply in flight: if snapshot still has the card, don't show it again", () => {
    const now = 50_000;
    const answeredAt = new Map([["a", now - 2_000]]); // Answered 2s ago, still within grace period

    const { decisions } = reconcileDecisions({
      prev: [card("b")],
      fresh: [card("a"), card("b")], // Desktop hasn't digested the reply yet, a is still in snapshot

      answeredAt,
      now,
    });
    expect(decisions.map((d) => d.id)).toEqual(["b"]);
  });

  it("Reply landed (card no longer in snapshot): clear from answeredAt, card stays gone", () => {
    const now = 50_000;
    const answeredAt = new Map([["a", now - 2_000]]);
    const { decisions, answeredAt: next } = reconcileDecisions({
      prev: [card("b")],
      fresh: [card("b")], // Desktop processed a, it's gone from snapshot

      answeredAt,
      now,
    });
    expect(decisions.map((d) => d.id)).toEqual(["b"]);
    expect(next.has("a")).toBe(false); // Delivery confirmed, suppression record cleared

  });

  it("Still in snapshot past grace period: conclude reply was lost, show card again to re-answer", () => {
    const now = 50_000;
    const answeredAt = new Map([["a", now - ANSWER_GRACE_MS - 1]]); // Long past grace period

    const { decisions, answeredAt: next } = reconcileDecisions({
      prev: [],
      fresh: [card("a")],
      answeredAt,
      now,
    });
    expect(decisions.map((d) => d.id)).toEqual(["a"]);
    expect(next.has("a")).toBe(false); // Suppression record cleared, no further suppression

  });
});

// ── Ghost agent empty snapshots ────────────────────────────────────────────────
// The relay broadcasts each phone request to **all** agents in the channel; the phone
// trusts the first reply. Testing 2026-08-21: besides the desktop, a second agent
// repeatedly joined/left the channel briefly (relay logs show 6 joins/leaves, ~20s each).
// It runs under a redirected FLEET_HOME, so pending_snapshot returns an empty list.
// Snapshots are authoritative whole-table replacements on the client, so cards got
// erased. They came back in the next reconcile after the ghost left — that's what the
// user saw: "card blinks off by itself, reappears 15 seconds later".
describe("reconcileDecisions — empty snapshot from unknown agent must not erase local cards", () => {

  it("Empty snapshot from mismatched source agent: ignore entirely, leave cards in place", () => {
    const prev = [card("a", 1_000)];
    const r = reconcileDecisions({
      prev,
      fresh: [],
      answeredAt: new Map(),
      now: 900_000, // Card is ancient; we're relying on source mismatch, not cooldown

      agentKey: "ghost-host/pid=999/home=/tmp/x",
      trustedAgentKey: "desk-host/pid=18953/home=/Users/hoveychen",
    });
    expect(r.decisions.map((d) => d.id)).toEqual(["a"]);
    expect(r.ignored?.reason).toBe("foreign-empty-snapshot");
    expect(r.ignored?.agentKey).toBe("ghost-host/pid=999/home=/tmp/x");
    // Ignored snapshots must not overwrite the trusted agent — otherwise a ghost could
    // seize the position, and the real next snapshot looks like a foreigner.

    expect(r.trustedAgentKey).toBe("desk-host/pid=18953/home=/Users/hoveychen");
  });

  it("Empty snapshot with no fingerprint (old desktop): new cards within cooldown still protected", () => {
    const now = 50_000;
    const r = reconcileDecisions({
      prev: [card("a", now - 1_000)], // Card just arrived 1s ago

      fresh: [],
      answeredAt: new Map(),
      now,
    });
    expect(r.decisions.map((d) => d.id)).toEqual(["a"]);
    expect(r.ignored?.reason).toBe("fresh-card-cooldown");
  });

  it("Empty snapshot from trusted agent itself: trust as usual, clear cards", () => {
    const key = "desk-host/pid=18953/home=/Users/hoveychen";
    const r = reconcileDecisions({
      prev: [card("a", 49_000)],
      fresh: [],
      answeredAt: new Map(),
      now: 50_000, // Card is very new, but source is trusted — cooldown shouldn't block authoritative "cleared"

      agentKey: key,
      trustedAgentKey: key,
    });
    expect(r.decisions).toEqual([]);
    expect(r.ignored).toBeUndefined();
  });

  it("Empty snapshot with no fingerprint, card past cooldown: still trust, won't lock forever", () => {
    const now = 50_000;
    const r = reconcileDecisions({
      prev: [card("a", now - EMPTY_SNAPSHOT_COOLDOWN_MS - 1)],
      fresh: [],
      answeredAt: new Map(),
      now,
    });
    expect(r.decisions).toEqual([]);
    expect(r.ignored).toBeUndefined();
  });

  // Testing 2026-08-24: desktop app rebuilt and restarted locally (12:54, pid 43541 →
  // 96862). The phone's "More" page listed the live desktop as "another 1 agent", with
  // trusted pinned to the dead old pid — the agent key includes pid. lsof confirmed only
  // one relay connection on that machine at that time, so this isn't impostor, it's
  // the same desktop's past life. We should gate on "can't read ~/.fleet" (judgment by
  // home), not pid.
  it("Desktop restart (same host/home/ver, pid changes): trust empty snapshot, don't mark as unknown agent", () => {
    const before = { host: "mbp", pid: 43541, home: "/Users/hoveychen", ver: "0.0.0" };
    const after = { host: "mbp", pid: 96862, home: "/Users/hoveychen", ver: "0.0.0" };
    const r = reconcileDecisions({
      prev: [card("a", 49_000)],
      fresh: [],
      answeredAt: new Map(),
      now: 50_000,
      agentKey: agentKeyOf(after),
      trustedAgentKey: agentKeyOf(before),
    });
    expect(r.ignored).toBeUndefined();
    expect(r.decisions).toEqual([]);
  });

  it("Changed home (ghost running under redirected FLEET_HOME): still mark as unknown agent", () => {
    const desk = { host: "mbp", pid: 43541, home: "/Users/hoveychen", ver: "0.0.0" };
    const ghost = { host: "mbp", pid: 96862, home: "/tmp/fleet-home-xyz", ver: "0.0.0" };
    const r = reconcileDecisions({
      prev: [card("a", 49_000)],
      fresh: [],
      answeredAt: new Map(),
      now: 900_000,
      agentKey: agentKeyOf(ghost),
      trustedAgentKey: agentKeyOf(desk),
    });
    expect(r.ignored?.reason).toBe("foreign-empty-snapshot");
    expect(r.decisions.map((d) => d.id)).toEqual(["a"]);
  });

  it("Non-empty snapshot: trust content, record source as trusted agent", () => {
    const r = reconcileDecisions({
      prev: [],
      fresh: [card("a")],
      answeredAt: new Map(),
      now: 10_000,
      agentKey: "desk-host/pid=18953/home=/Users/hoveychen",
    });
    expect(r.decisions.map((d) => d.id)).toEqual(["a"]);
    expect(r.trustedAgentKey).toBe("desk-host/pid=18953/home=/Users/hoveychen");
    expect(r.ignored).toBeUndefined();
  });
});
