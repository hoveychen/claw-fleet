import { describe, expect, it } from "vitest";
import {
  aggregateDecisions,
  aggregateSessions,
  allDecisionsLoaded,
  anyAgentOnline,
  anyConnected,
  devicesReducer,
  emptyDeviceState,
  itemKey,
  offlineDeviceCount,
  totalUsage,
  usageByDevice,
  worstCongestion,
  type DeviceStates,
} from "./deviceRuntime";
import type { DecisionRequest, SessionInfo, TodayUsage } from "./types";

const A = "dev-a";
const B = "dev-b";
const ORDER = [A, B];

function req(id: string): DecisionRequest {
  return { id } as DecisionRequest;
}

function session(id: string): SessionInfo {
  return { id, workspacePath: "/w" } as SessionInfo;
}

function usage(costUsd: number): TodayUsage {
  return {
    date: "2026-09-01",
    inputTokens: 10,
    outputTokens: 5,
    costUsd,
    agentCostUsd: costUsd,
    fleetCostUsd: 0,
    sessionCount: 1,
  };
}

/** Apply a sequence of actions in order. */
function run(
  actions: Array<Parameters<typeof devicesReducer>[1]>,
  from: DeviceStates = {},
): DeviceStates {
  return actions.reduce((s, a) => devicesReducer(s, a), from);
}

describe("devicesReducer", () => {
  it("keeps each device's cards apart", () => {
    const states = run([
      { deviceId: A, type: "decisionCreated", kind: "guard", request: req("g1"), now: 1 },
      { deviceId: B, type: "decisionCreated", kind: "guard", request: req("g9"), now: 2 },
    ]);
    expect(states[A].decisions.map((d) => d.id)).toEqual(["g1"]);
    expect(states[B].decisions.map((d) => d.id)).toEqual(["g9"]);
  });

  // Decision card IDs are unique only per device. Cards with the same ID on two
  // machines are two different cards; resolving one must never erase the other.
  it("resolving a card on one device leaves the same id on the other alone", () => {
    const states = run([
      { deviceId: A, type: "decisionCreated", kind: "guard", request: req("g1"), now: 1 },
      { deviceId: B, type: "decisionCreated", kind: "guard", request: req("g1"), now: 1 },
      { deviceId: A, type: "decisionResolved", id: "g1" },
    ]);
    expect(states[A].decisions).toEqual([]);
    expect(states[B].decisions.map((d) => d.id)).toEqual(["g1"]);
  });

  it("ignores a duplicate live frame for a card already held", () => {
    const states = run([
      { deviceId: A, type: "decisionCreated", kind: "guard", request: req("g1"), now: 1 },
      { deviceId: A, type: "decisionCreated", kind: "guard", request: req("g1"), now: 2 },
    ]);
    expect(states[A].decisions).toHaveLength(1);
    expect(states[A].decisions[0].arrivedAt).toBe(1);
  });

  it("a live frame retires the skeleton even before the first snapshot", () => {
    const states = run([
      { deviceId: A, type: "decisionCreated", kind: "guard", request: req("g1"), now: 1 },
    ]);
    expect(states[A].decisionsLoaded).toBe(true);
  });

  // Answering is optimistic: the card vanishes immediately, and we record the
  // timestamp so a lagging snapshot cannot resurrect it.
  it("an answered card stays gone when a lagging snapshot still lists it", () => {
    const states = run([
      { deviceId: A, type: "decisionCreated", kind: "guard", request: req("g1"), now: 1 },
      { deviceId: A, type: "answered", id: "g1", now: 100 },
      {
        deviceId: A,
        type: "snapshot",
        fresh: [{ kind: "guard", id: "g1", request: req("g1"), arrivedAt: 200 }],
        agent: { host: "mac", home: "/h", ver: "1" },
        now: 200,
      },
    ]);
    expect(states[A].decisions).toEqual([]);
    expect(states[A].answeredAt.has("g1")).toBe(true);
  });

  it("records who served each snapshot", () => {
    const states = run([
      {
        deviceId: A,
        type: "snapshot",
        fresh: [{ kind: "guard", id: "g1", request: req("g1"), arrivedAt: 1 }],
        agent: { host: "mac", home: "/h", ver: "1" },
        now: 1,
      },
    ]);
    expect(states[A].snapshotSources).toHaveLength(1);
    expect(states[A].trustedAgentKey).toBeDefined();
  });

  it("a cached list never overwrites live data", () => {
    const states = run([
      { deviceId: A, type: "sessions", list: [session("live")] },
      { deviceId: A, type: "cachedSessions", list: [session("stale")] },
    ]);
    expect(states[A].sessions.map((s) => s.id)).toEqual(["live"]);
  });

  it("a cached list paints when nothing live has landed", () => {
    const states = run([{ deviceId: A, type: "cachedSessions", list: [session("stale")] }]);
    expect(states[A].sessions.map((s) => s.id)).toEqual(["stale"]);
    // …but it must not claim the first live frame arrived, or the task page
    // would stop labelling itself as showing cache.
    expect(states[A].sessionsLoaded).toBe(false);
  });

  it("detach drops the device's whole state", () => {
    let states = run([{ deviceId: A, type: "status", connected: true }]);
    states = devicesReducer(states, { deviceId: A, type: "detach" });
    expect(states[A]).toBeUndefined();
  });

  it("returns the same object when nothing changed (no needless re-render)", () => {
    const first = run([{ deviceId: A, type: "status", connected: true }]);
    const again = devicesReducer(first, { deviceId: A, type: "status", connected: true });
    expect(again).toBe(first);
  });

  it("congestion follows rtt and reconnect signals per device", () => {
    const states = run([
      { deviceId: A, type: "rtt", sample: { totalMs: 4_000, phoneRelayMs: null, desktopHandleMs: null } },
      { deviceId: B, type: "rtt", sample: { totalMs: 100, phoneRelayMs: null, desktopHandleMs: null } },
    ]);
    expect(states[A].congestion).not.toBe("good");
    expect(states[B].congestion).toBe("good");
  });
});

describe("aggregation", () => {
  const states = run([
    { deviceId: A, type: "decisionCreated", kind: "guard", request: req("g1"), now: 10 },
    { deviceId: B, type: "decisionCreated", kind: "fleet-ask", request: req("f1"), now: 5 },
    { deviceId: A, type: "sessions", list: [session("s1")] },
    { deviceId: B, type: "sessions", list: [session("s2")] },
  ]);

  it("merges cards from every device, oldest first", () => {
    const merged = aggregateDecisions(states, ORDER);
    expect(merged.map((d) => [d.deviceId, d.id])).toEqual([
      [B, "f1"],
      [A, "g1"],
    ]);
  });

  it("tags every merged item with its owning device", () => {
    expect(aggregateSessions(states, ORDER).map((s) => [s.deviceId, s.id])).toEqual([
      [A, "s1"],
      [B, "s2"],
    ]);
  });

  // When arriving at the same time, sort stably by device order to prevent list
  // thrashing on every refresh.
  it("breaks arrival ties by device order", () => {
    const tied = run([
      { deviceId: A, type: "decisionCreated", kind: "guard", request: req("x"), now: 7 },
      { deviceId: B, type: "decisionCreated", kind: "guard", request: req("y"), now: 7 },
    ]);
    expect(aggregateDecisions(tied, ORDER).map((d) => d.deviceId)).toEqual([A, B]);
    expect(aggregateDecisions(tied, [B, A]).map((d) => d.deviceId)).toEqual([B, A]);
  });

  it("composite keys keep same-id cards from different devices distinct", () => {
    expect(itemKey(A, "g1")).not.toBe(itemKey(B, "g1"));
  });
});

describe("header rollups", () => {
  it("one device offline does not make the whole app look offline", () => {
    const states = run([
      { deviceId: A, type: "status", connected: true },
      { deviceId: A, type: "agentOnline", online: true },
      { deviceId: B, type: "status", connected: false },
    ]);
    expect(anyConnected(states, ORDER)).toBe(true);
    expect(anyAgentOnline(states, ORDER)).toBe(true);
  });

  it("the congestion light shows the worst link", () => {
    const states = run([
      { deviceId: A, type: "rtt", sample: { totalMs: 50, phoneRelayMs: null, desktopHandleMs: null } },
      { deviceId: B, type: "rtt", sample: { totalMs: 9_000, phoneRelayMs: null, desktopHandleMs: null } },
    ]);
    expect(worstCongestion(states, ORDER)).toBe("congested");
  });

  it("today's spend sums across devices", () => {
    const states = run([
      { deviceId: A, type: "usage", usage: usage(1.5) },
      { deviceId: B, type: "usage", usage: usage(2.25) },
    ]);
    expect(totalUsage(states, ORDER)?.costUsd).toBeCloseTo(3.75);
    expect(totalUsage(states, ORDER)?.sessionCount).toBe(2);
  });

  // "Don't know yet" and "spent nothing today" are different: we can't display $0.00
  // when no device has reported.
  it("spend is null until at least one device reports", () => {
    expect(totalUsage({}, ORDER)).toBeNull();
  });

  // The aggregate can't answer "which device is burning money", so we must also be
  // able to split the same state back per device.
  it("today's spend also splits back per device, in switcher order", () => {
    const states = run([
      { deviceId: A, type: "usage", usage: usage(1.5) },
      { deviceId: B, type: "usage", usage: usage(2.25) },
    ]);
    const rows = usageByDevice(states, ORDER);
    expect(rows.map((r) => r.id)).toEqual(ORDER);
    expect(rows.map((r) => r.usage?.costUsd)).toEqual([1.5, 2.25]);
  });

  // An unreported device must still occupy a row (null), otherwise the UI can't show
  // "the aggregate is missing it".
  it("a device that has not reported keeps its row with a null usage", () => {
    const states = run([{ deviceId: A, type: "usage", usage: usage(1.5) }]);
    expect(usageByDevice(states, ORDER)[1]).toEqual({ id: B, usage: null });
  });

  it("the skeleton only retires once every online device has answered once", () => {
    const online = run([
      { deviceId: A, type: "status", connected: true },
      { deviceId: A, type: "agentOnline", online: true },
      { deviceId: B, type: "status", connected: true },
      { deviceId: B, type: "agentOnline", online: true },
    ]);
    const partial = run(
      [
        {
          deviceId: A,
          type: "snapshot",
          fresh: [],
          agent: { host: "mac", home: "/h", ver: "1" },
          now: 1,
        },
      ],
      online,
    );
    expect(allDecisionsLoaded(partial, ORDER)).toBe(false);
    const both = devicesReducer(partial, {
      deviceId: B,
      type: "snapshot",
      fresh: [],
      agent: { host: "linux", home: "/h", ver: "1" },
      now: 1,
    });
    expect(allDecisionsLoaded(both, ORDER)).toBe(true);
  });

  // An offline device never sends a snapshot. If it holds the gate, the skeleton
  // spins forever and the "desktop offline / no pending decisions" hint never shows.
  it("an offline device does not hold the skeleton gate", () => {
    const states = run([
      { deviceId: A, type: "status", connected: true },
      { deviceId: A, type: "agentOnline", online: true },
      {
        deviceId: A,
        type: "snapshot",
        fresh: [],
        agent: { host: "mac", home: "/h", ver: "1" },
        now: 1,
      },
      // B relays in, but its desktop is offline — snapshot never arrives.
      { deviceId: B, type: "status", connected: true },
    ]);
    expect(states[B].decisionsLoaded).toBe(false);
    expect(allDecisionsLoaded(states, ORDER)).toBe(true);
  });

  // "All answered" only applies to online devices, so the empty state must report
  // the count of offline ones.
  it("counts the devices whose desktop is offline", () => {
    const states = run([
      { deviceId: A, type: "status", connected: true },
      { deviceId: A, type: "agentOnline", online: true },
      { deviceId: B, type: "status", connected: true },
    ]);
    expect(offlineDeviceCount(states, ORDER)).toBe(1);
    expect(
      offlineDeviceCount(
        devicesReducer(states, { deviceId: B, type: "agentOnline", online: true }),
        ORDER,
      ),
    ).toBe(0);
    // Devices that have never attached count as offline — clearly not pushing cards.
    expect(offlineDeviceCount({}, ORDER)).toBe(2);
  });

  // When all devices are offline, don't spin the skeleton either — show the
  // "desktop offline" hint instead.
  it("all-offline reads as loaded so the offline hint can render", () => {
    const states = run([
      { deviceId: A, type: "status", connected: true },
      { deviceId: B, type: "status", connected: true },
    ]);
    expect(allDecisionsLoaded(states, ORDER)).toBe(true);
  });

  it("an unknown device reads as the empty state, never undefined", () => {
    expect(emptyDeviceState().decisions).toEqual([]);
    expect(anyConnected({}, ORDER)).toBe(false);
  });
});

// ── Dead-link grading ───────────────────────────────────────────────────────
describe("the header signal reflects requests that never came back", () => {
  const sample = { totalMs: 120, phoneRelayMs: 40, desktopHandleMs: 20 };

  it("stops claiming a healthy link once two requests go unanswered", () => {
    // The reported symptom: one good round trip early on, then the link dies.
    // Before this the light kept showing that first measurement forever.
    const states = run([
      { deviceId: A, type: "rtt", sample },
      { deviceId: A, type: "requestTimeout" },
      { deviceId: A, type: "requestTimeout" },
    ]);
    expect(states[A].congestion).toBe("stalled");
  });

  it("recovers as soon as a reply comes back", () => {
    const states = run([
      { deviceId: A, type: "requestTimeout" },
      { deviceId: A, type: "requestTimeout" },
      { deviceId: A, type: "rtt", sample },
    ]);
    expect(states[A].congestion).toBe("good");
    expect(states[A].consecutiveTimeouts).toBe(0);
  });

  it("grades a condemned socket as stalled without waiting for two timeouts", () => {
    // The probe proved the link dead, so this is evidence, not inference.
    const states = run([{ deviceId: A, type: "deadLink" }]);
    expect(states[A].congestion).toBe("stalled");
  });

  it("lets one stalled device dominate the header", () => {
    const states = run([
      { deviceId: A, type: "rtt", sample },
      { deviceId: B, type: "deadLink" },
    ]);
    expect(worstCongestion(states, [A, B])).toBe("stalled");
  });
});
