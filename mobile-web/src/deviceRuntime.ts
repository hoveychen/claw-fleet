// Runtime state per device, plus selectors that merge them into one view.
//
// Before this, these states lived as a flat useState in App: one connected, one sessions,
// one decisions, a pair of refs tracking "who's the trusted agent". That approach hardwired
// "single source of truth" into the component — multi-device needs each state per device.
//
// Deliberately write state transitions as a **pure reducer** rather than scattered setState in
// handlers:
//   * The transition rules themselves have substance (snapshot reconcile, trusted-agent logic,
//     optimistic-answer suppression window, congestion grading) that deserves unit tests,
//     and these rules don't need React.
//   * The component layer thus becomes only "translate transport callbacks to actions", with no branches.
//
// Division of labor with devices.ts: that file covers "which devices are paired" (persistent),
// this file covers "what's happening with them right now" (in-memory, per-process).

import {
  computeCongestion,
  RECONNECT_WINDOW_MS,
  splitRtt,
  type Congestion,
  type RttSplit,
} from "./connQuality";
import { agentKeyOf, reconcileDecisions } from "./decisionReconcile";
import { recordSnapshotSource, type SnapshotSource } from "./snapshotSources";
import type {
  AgentFingerprint,
  DecisionKind,
  DecisionRequest,
  PendingDecision,
  SessionInfo,
  TodayUsage,
} from "./types";

/** Complete runtime state for one device right now. */
export interface DeviceRuntimeState {
  /** Connectivity from this device to the relay. */
  connected: boolean;
  /** Whether the desktop is online. */
  agentOnline: boolean;
  sessions: SessionInfo[];
  /** Whether the first sessions frame arrived — distinguishes "waiting for first" from "got empty". */
  sessionsLoaded: boolean;
  decisions: PendingDecision[];
  decisionsLoaded: boolean;
  todayUsage: TodayUsage | null;
  /** Latest sessions frame type (full/delta) and cumulative counts (diagnostic). */
  sessionsFrame: { last: "full" | "delta" | null; full: number; delta: number };
  rttSplit: RttSplit | null;
  congestion: Congestion;
  authError: string | null;
  /** Every agent that's returned a snapshot (diagnostic: normally just one). */
  snapshotSources: SnapshotSource[];
  /** Card IDs answered on this phone but replies still in flight, mapped to timestamps. */
  answeredAt: Map<string, number>;
  /** Agent fingerprint trusted as "this is the desktop". */
  trustedAgentKey?: string;
  /** Latest round-trip latency (raw signal for congestion grading). */
  lastRttMs: number | null;
  /** Recent reconnects within the window. */
  recentReconnects: number[];
  /** Requests that timed out since the last one that came back. The only
   *  signal that sees a link which has stopped answering entirely — the other
   *  two are derived from successful round trips and from browser-reported
   *  closes, neither of which happens on a half-open socket. */
  consecutiveTimeouts: number;
}

export function emptyDeviceState(): DeviceRuntimeState {
  return {
    connected: false,
    agentOnline: false,
    sessions: [],
    sessionsLoaded: false,
    decisions: [],
    decisionsLoaded: false,
    todayUsage: null,
    sessionsFrame: { last: null, full: 0, delta: 0 },
    rttSplit: null,
    congestion: "good",
    authError: null,
    snapshotSources: [],
    answeredAt: new Map(),
    lastRttMs: null,
    recentReconnects: [],
    consecutiveTimeouts: 0,
  };
}

/** Device id → its runtime state. */
export type DeviceStates = Record<string, DeviceRuntimeState>;

export type DeviceAction =
  /** Device enters operation (idempotent: already active returns unchanged). */
  | { type: "attach" }
  /** Device is removed, state discarded with it. */
  | { type: "detach" }
  | { type: "status"; connected: boolean }
  | { type: "agentOnline"; online: boolean }
  | { type: "sessions"; list: SessionInfo[] }
  /** Cold-start cache draw: only works before live data arrives, doesn't overwrite fresh data. */
  | { type: "cachedSessions"; list: SessionInfo[] }
  | { type: "sessionsKind"; kind: "full" | "delta" }
  | { type: "decisionCreated"; kind: DecisionKind; request: DecisionRequest; now: number }
  | { type: "decisionResolved"; id: string }
  /** This phone just answered a card: optimistically remove and record timestamp to suppress late snapshot resurrection. */
  | { type: "answered"; id: string; now: number }
  | {
      type: "snapshot";
      fresh: PendingDecision[];
      agent: AgentFingerprint | undefined;
      now: number;
    }
  | { type: "usage"; usage: TodayUsage }
  | { type: "rtt"; sample: { totalMs: number; phoneRelayMs: number | null; desktopHandleMs: number | null } }
  | { type: "reconnect"; now: number }
  /** A request went out and never came back. */
  | { type: "requestTimeout" }
  /** The transport's own liveness probe condemned the socket. Not a guess from
   *  a count of failures — the link was proven dead, so grade it directly. */
  | { type: "deadLink" }
  | { type: "authError"; message: string };

/** State transition for a single device. */
export function deviceReducer(
  state: DeviceRuntimeState,
  action: DeviceAction,
): DeviceRuntimeState {
  switch (action.type) {
    case "attach":
    case "detach":
      return state;
    case "status":
      return state.connected === action.connected
        ? state
        : { ...state, connected: action.connected };
    case "agentOnline":
      return state.agentOnline === action.online
        ? state
        : { ...state, agentOnline: action.online };
    case "sessions":
      return { ...state, sessions: action.list, sessionsLoaded: true };
    case "cachedSessions":
      // Once live snapshot arrives, don't touch it — cache is always older than live.
      return state.sessionsLoaded || state.sessions.length > 0
        ? state
        : { ...state, sessions: action.list };
    case "sessionsKind":
      return {
        ...state,
        sessionsFrame: {
          last: action.kind,
          full: state.sessionsFrame.full + (action.kind === "full" ? 1 : 0),
          delta: state.sessionsFrame.delta + (action.kind === "delta" ? 1 : 0),
        },
      };
    case "decisionCreated": {
      // One frame of live decision cards proves the pipeline is delivering — even if
      // the first snapshot hasn't arrived, the skeleton should disappear.
      if (!action.request?.id) return state;
      if (state.decisions.some((d) => d.id === action.request.id)) {
        return state.decisionsLoaded ? state : { ...state, decisionsLoaded: true };
      }
      return {
        ...state,
        decisionsLoaded: true,
        decisions: [
          ...state.decisions,
          {
            kind: action.kind,
            id: action.request.id,
            request: action.request,
            arrivedAt: action.now,
          },
        ],
      };
    }
    case "decisionResolved": {
      // Resolved authoritatively (desktop or another phone) — this phone's "answer in flight" record is void.
      const answeredAt = new Map(state.answeredAt);
      answeredAt.delete(action.id);
      return {
        ...state,
        answeredAt,
        decisions: state.decisions.filter((d) => d.id !== action.id),
      };
    }
    case "answered": {
      const answeredAt = new Map(state.answeredAt);
      answeredAt.set(action.id, action.now);
      return {
        ...state,
        answeredAt,
        decisions: state.decisions.filter((d) => d.id !== action.id),
      };
    }
    case "snapshot": {
      const agentKey = agentKeyOf(action.agent);
      const { decisions, answeredAt, ignored, trustedAgentKey } = reconcileDecisions({
        prev: state.decisions,
        fresh: action.fresh,
        answeredAt: state.answeredAt,
        now: action.now,
        agentKey,
        trustedAgentKey: state.trustedAgentKey,
      });
      return {
        ...state,
        decisions,
        answeredAt,
        trustedAgentKey,
        decisionsLoaded: true,
        snapshotSources: recordSnapshotSource(state.snapshotSources, {
          key: agentKey,
          agent: action.agent,
          at: action.now,
          trusted: !!agentKey && agentKey === trustedAgentKey,
          ignored: !!ignored,
        }),
      };
    }
    case "usage":
      return { ...state, todayUsage: action.usage };
    case "rtt": {
      // A reply arrived, so whatever was unanswered before is no longer
      // evidence of anything: reset the run rather than letting one old
      // timeout hold the light down over a link that plainly works.
      const lastRttMs = action.sample.totalMs;
      return {
        ...state,
        lastRttMs,
        rttSplit: splitRtt(action.sample),
        consecutiveTimeouts: 0,
        congestion: computeCongestion(lastRttMs, state.recentReconnects.length, 0),
      };
    }
    case "requestTimeout": {
      const consecutiveTimeouts = state.consecutiveTimeouts + 1;
      return {
        ...state,
        consecutiveTimeouts,
        congestion: computeCongestion(
          state.lastRttMs,
          state.recentReconnects.length,
          consecutiveTimeouts,
        ),
      };
    }
    case "deadLink":
      return { ...state, congestion: "stalled" };
    case "reconnect": {
      const recentReconnects = [...state.recentReconnects, action.now].filter(
        (ts) => action.now - ts < RECONNECT_WINDOW_MS,
      );
      return {
        ...state,
        recentReconnects,
        congestion: computeCongestion(
          state.lastRttMs,
          recentReconnects.length,
          state.consecutiveTimeouts,
        ),
      };
    }
    case "authError":
      return { ...state, authError: action.message };
  }
}

/** State transition for the whole book. Action carries which device. */
export function devicesReducer(
  states: DeviceStates,
  action: DeviceAction & { deviceId: string },
): DeviceStates {
  const { deviceId, ...rest } = action;
  if (rest.type === "detach") {
    if (!(deviceId in states)) return states;
    const next = { ...states };
    delete next[deviceId];
    return next;
  }
  const before = states[deviceId] ?? emptyDeviceState();
  const after = deviceReducer(before, rest as DeviceAction);
  if (after === before && deviceId in states) return states;
  return { ...states, [deviceId]: after };
}

// ── Aggregated selectors ───────────────────────────────────────────────────────────
//
// Inbox and task list are **merged cross-device**, so every item must carry which device
// it belongs to: drill-down uses that device's transport, reply goes back there, badges
// show that device's name. IDs are unique only per device, so the UI always uses
// (deviceId, id) composite keys.

/** One item in merged list: the object plus its device. */
export type WithDevice<T> = T & { deviceId: string };

/** Composite key. When IDs collide across devices, this is the only way to distinguish
 *  two cards — React key, dedup, "which card did I just answer" all use it. */
export function itemKey(deviceId: string, id: string): string {
  return `${deviceId}::${id}`;
}

/** All pending-decision cards across devices, sorted by arrival time (newest last, matching
 *  single-device era). `order` gives device order for stable sort when arrival times match. */
export function aggregateDecisions(
  states: DeviceStates,
  order: string[],
): Array<WithDevice<PendingDecision>> {
  const out: Array<WithDevice<PendingDecision>> = [];
  order.forEach((deviceId, rank) => {
    for (const d of states[deviceId]?.decisions ?? []) out.push({ ...d, deviceId });
    void rank;
  });
  return out.sort((a, b) => {
    if (a.arrivedAt !== b.arrivedAt) return a.arrivedAt - b.arrivedAt;
    return order.indexOf(a.deviceId) - order.indexOf(b.deviceId);
  });
}

/** All sessions across devices, reverse-sorted by recent activity (matches original task page). */
export function aggregateSessions(
  states: DeviceStates,
  order: string[],
): Array<WithDevice<SessionInfo>> {
  const out: Array<WithDevice<SessionInfo>> = [];
  for (const deviceId of order) {
    for (const s of states[deviceId]?.sessions ?? []) out.push({ ...s, deviceId });
  }
  return out;
}

/** Is any device's agent online? Header light uses this: one offline shouldn't make the whole
 *  UI show "offline" when others are pushing data normally. */
export function anyAgentOnline(states: DeviceStates, order: string[]): boolean {
  return order.some((id) => states[id]?.agentOnline);
}

/** Is any device connected to the relay? */
export function anyConnected(states: DeviceStates, order: string[]): boolean {
  return order.some((id) => states[id]?.connected);
}

/** Total spend across all devices today; null if none have reported (not 0, which shows "no spend",
 *  different from "unknown"). */
export function totalUsage(states: DeviceStates, order: string[]): TodayUsage | null {
  let sum: TodayUsage | null = null;
  for (const id of order) {
    const u = states[id]?.todayUsage;
    if (!u) continue;
    if (!sum) {
      // Take the first device's fields as-is (for non-numeric fields like date), then accumulate numbers.
      sum = { ...u };
      continue;
    }
    sum = {
      ...sum,
      inputTokens: sum.inputTokens + u.inputTokens,
      outputTokens: sum.outputTokens + u.outputTokens,
      costUsd: sum.costUsd + u.costUsd,
      agentCostUsd: sum.agentCostUsd + u.agentCostUsd,
      fleetCostUsd: sum.fleetCostUsd + u.fleetCostUsd,
      sessionCount: sum.sessionCount + u.sessionCount,
    };
  }
  return sum;
}

/** This device's contribution to the total. `usage` is `null` = device hasn't reported yet
 *  (offline/just connected), different from "spent zero today"; renderer distinguishes them. */
export interface DeviceUsage {
  id: string;
  usage: TodayUsage | null;
}

/** Break down total usage to one row per device. Order follows `order`, matching the device
 *  switcher. Exists because `totalUsage` sums all devices into one number, which doesn't answer
 *  "which device is spending" — especially when they're logged into different accounts. */
export function usageByDevice(states: DeviceStates, order: string[]): DeviceUsage[] {
  return order.map((id) => ({ id, usage: states[id]?.todayUsage ?? null }));
}

/** Worst congestion grade — the header has one light, and users feel the slowest link. */
export function worstCongestion(states: DeviceStates, order: string[]): Congestion {
  let level: Congestion = "good";
  for (const id of order) {
    const c = states[id]?.congestion ?? "good";
    if (c === "stalled") return "stalled";
    if (c === "congested") level = "congested";
    else if (c === "fair" && level === "good") level = "fair";
  }
  return level;
}

/** First-screen skeleton gate: "still waiting" only when devices **could** return snapshot
 *  but haven't yet.
 *
 *  Only `connected && agentOnline` devices guard this gate. A paired-but-offline device never
 *  returns a snapshot; letting it guard means the skeleton spins forever — with multiple devices
 *  where one is offline, the decision page gets stuck spinning, unable to show even
 *  "desktop offline / no decisions". */
export function allDecisionsLoaded(states: DeviceStates, order: string[]): boolean {
  if (order.length === 0) return false;
  return !order.some((id) => {
    const s = states[id];
    if (!s || s.decisionsLoaded) return false;
    return s.connected && s.agentOnline;
  });
}

/** Count of offline devices (agent not online). Merged inbox shows only online devices' cards,
 *  so "all answered" is incomplete when devices are offline — this number completes the picture. */
export function offlineDeviceCount(states: DeviceStates, order: string[]): number {
  return order.filter((id) => !states[id]?.agentOnline).length;
}

export function anySessionsLoaded(states: DeviceStates, order: string[]): boolean {
  return order.some((id) => states[id]?.sessionsLoaded);
}
