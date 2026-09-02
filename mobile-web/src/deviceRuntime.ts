// 每台设备的运行时状态,以及把它们并成一份的选择器。
//
// 在此之前这些状态是 App 里一把扁平的 useState:一个 connected、一个 sessions、
// 一个 decisions、一对 ref 记着「谁是可信 agent」。那套写法把「只有一个数据源」
// 焊死进了组件——多设备之后每一项都要按设备各来一份。
//
// 这里刻意把状态迁移写成**纯 reducer** 而不是散在 handler 里的 setState:
//   * 迁移规则本身是有内容的(快照对账、可信 agent 判定、乐观答复的抑制窗口、
//     拥塞判级),值得被单测钉住,而这些规则不需要 React 才能跑。
//   * 组件那一层因此只剩下「把传输层的回调翻译成 action」,那部分没有分支。
//
// 与 devices.ts 的分工:那边是**配对了哪些设备**(持久化),这里是**这些设备此刻
// 怎么样**(内存,进程内)。

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

/** 一台设备此刻的全部运行时状态。 */
export interface DeviceRuntimeState {
  /** 这台设备到中转的连通性。 */
  connected: boolean;
  /** 那台桌面端在不在线。 */
  agentOnline: boolean;
  sessions: SessionInfo[];
  /** 首份 sessions 帧是否到过 —— 用来区分「还在等第一帧」与「推过了,确实空」。 */
  sessionsLoaded: boolean;
  decisions: PendingDecision[];
  decisionsLoaded: boolean;
  todayUsage: TodayUsage | null;
  /** 最近一帧 sessions 是全量还是增量,以及各自累计数(诊断用)。 */
  sessionsFrame: { last: "full" | "delta" | null; full: number; delta: number };
  rttSplit: RttSplit | null;
  congestion: Congestion;
  authError: string | null;
  /** 回过快照的每一个 agent(诊断用:正常只有一个)。 */
  snapshotSources: SnapshotSource[];
  /** 在本机答过、但答复还在路上的卡 id → 时间戳。 */
  answeredAt: Map<string, number>;
  /** 被采信为「就是这台桌面端」的 agent 指纹。 */
  trustedAgentKey?: string;
  /** 最近一次往返耗时(拥塞判级用的原始信号)。 */
  lastRttMs: number | null;
  /** 最近一个窗口内的重连时刻。 */
  recentReconnects: number[];
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
  };
}

/** 设备 id → 它的运行时状态。 */
export type DeviceStates = Record<string, DeviceRuntimeState>;

export type DeviceAction =
  /** 这台设备进入运行(幂等:已在册就原样返回)。 */
  | { type: "attach" }
  /** 这台设备被移除,连同它的状态一起丢掉。 */
  | { type: "detach" }
  | { type: "status"; connected: boolean }
  | { type: "agentOnline"; online: boolean }
  | { type: "sessions"; list: SessionInfo[] }
  /** 冷启动时从缓存画一笔:只在还没有实时数据时生效,免得把刚到的真数据盖掉。 */
  | { type: "cachedSessions"; list: SessionInfo[] }
  | { type: "sessionsKind"; kind: "full" | "delta" }
  | { type: "decisionCreated"; kind: DecisionKind; request: DecisionRequest; now: number }
  | { type: "decisionResolved"; id: string }
  /** 本机刚答完一张卡:先乐观移除,并记下时间戳压住迟到快照的复活。 */
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
  | { type: "authError"; message: string };

/** 单台设备的状态迁移。 */
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
      // 实时快照已经到过就不动它 —— 缓存永远比实时旧。
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
      // 一帧实时决策卡本身就证明管道在投递 —— 即使首份快照还没回来,骨架屏也
      // 该退场。
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
      // 权威解决(桌面端或另一台手机)——本机那条「答复在路上」的记账作废。
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
      const lastRttMs = action.sample.totalMs;
      return {
        ...state,
        lastRttMs,
        rttSplit: splitRtt(action.sample),
        congestion: computeCongestion(lastRttMs, state.recentReconnects.length),
      };
    }
    case "reconnect": {
      const recentReconnects = [...state.recentReconnects, action.now].filter(
        (ts) => action.now - ts < RECONNECT_WINDOW_MS,
      );
      return {
        ...state,
        recentReconnects,
        congestion: computeCongestion(state.lastRttMs, recentReconnects.length),
      };
    }
    case "authError":
      return { ...state, authError: action.message };
  }
}

/** 整本簿子的迁移。action 带上是哪一台。 */
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

// ── 聚合选择器 ───────────────────────────────────────────────────────────────
//
// 收件箱与任务列表是**跨设备合并**的,所以每一项都必须带上它属于哪一台:下钻要
// 用那一台的 transport,答复要发回那一台,徽标也要显示那一台的名字。id 只在单机
// 内唯一,所以 UI 侧一律用 (deviceId, id) 复合键。

/** 合并列表里的一项:原对象 + 归属设备。 */
export type WithDevice<T> = T & { deviceId: string };

/** 复合键。跨设备撞 id 时它是唯一能区分两张卡的东西 —— React key、去重、
 *  「刚答过的是哪一张」全都用它。 */
export function itemKey(deviceId: string, id: string): string {
  return `${deviceId}::${id}`;
}

/** 所有设备的待决策卡,按到达时间排(新的在后,与单设备时代一致)。
 *  `order` 给出设备顺序,用于同一时刻到达时的稳定排序。 */
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

/** 所有设备的会话,按最近活动时间倒序(与任务页原本的排序口径一致)。 */
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

/** 有没有任何一台在线。头部那盏灯用它:一台离线不该让整个界面显示「离线」,
 *  因为其他几台还在正常推数据。 */
export function anyAgentOnline(states: DeviceStates, order: string[]): boolean {
  return order.some((id) => states[id]?.agentOnline);
}

/** 有没有任何一台连着中转。 */
export function anyConnected(states: DeviceStates, order: string[]): boolean {
  return order.some((id) => states[id]?.connected);
}

/** 全部设备的今日花费之和;一台都没报过就是 null(而不是 0 —— 那会显示成
 *  「今天没花钱」,和「还不知道」是两回事)。 */
export function totalUsage(states: DeviceStates, order: string[]): TodayUsage | null {
  let sum: TodayUsage | null = null;
  for (const id of order) {
    const u = states[id]?.todayUsage;
    if (!u) continue;
    if (!sum) {
      // 第一台的那份原样收下(date 之类的非数值字段取它的),后面的只累加数值。
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

/** 最差的那一档拥塞 —— 头部只有一盏灯,而用户感觉到的是最卡的那条链路。 */
export function worstCongestion(states: DeviceStates, order: string[]): Congestion {
  let level: Congestion = "good";
  for (const id of order) {
    const c = states[id]?.congestion ?? "good";
    if (c === "congested") return "congested";
    if (c === "fair") level = "fair";
  }
  return level;
}

/** 首屏骨架屏的闸门:每台都还没收到过首份快照时才算「还在等」。 */
export function allDecisionsLoaded(states: DeviceStates, order: string[]): boolean {
  return order.length > 0 && order.every((id) => states[id]?.decisionsLoaded);
}

export function anySessionsLoaded(states: DeviceStates, order: string[]): boolean {
  return order.some((id) => states[id]?.sessionsLoaded);
}
