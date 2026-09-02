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
  totalUsage,
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

/** 依次施加一串 action。 */
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

  // 决策卡 id 只在单机内唯一。两台机器上同号的卡是两张不同的卡，解决其中一张
  // 绝不该把另一张也抹掉。
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

  // 本机答复是乐观的：卡立刻消失，并记下时间戳，好让迟到的快照不能把它复活。
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

  // 同一时刻到达时按设备顺序稳定排，免得列表每次刷新自己抖动。
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

  // 「还不知道」和「今天没花钱」是两回事：一台都没报过时不能显示 $0.00。
  it("spend is null until at least one device reports", () => {
    expect(totalUsage({}, ORDER)).toBeNull();
  });

  it("the skeleton only retires once every device has answered once", () => {
    const partial = run([
      {
        deviceId: A,
        type: "snapshot",
        fresh: [],
        agent: { host: "mac", home: "/h", ver: "1" },
        now: 1,
      },
    ]);
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

  it("an unknown device reads as the empty state, never undefined", () => {
    expect(emptyDeviceState().decisions).toEqual([]);
    expect(anyConnected({}, ORDER)).toBe(false);
  });
});
