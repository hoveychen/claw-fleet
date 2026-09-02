// 一台设备的连接:建传输层、把它的回调翻译成 action、跑那几条只与这台有关的
// 轮询。不渲染任何东西。
//
// 为什么是**组件**而不是 App 里的一个循环:hook 不能在数组上循环调用(设备数一变
// 就错位)。每台设备渲染一个 `<DeviceConnection key={id}>` 是 React 里表达「一份
// 独立生命周期」的标准写法 —— 设备被移除时 key 消失,它的 effect 清理函数自然
// 关掉那条 socket,不需要任何手工的销毁登记。
//
// 状态不留在这里:它 dispatch 到 App 的那份 `DeviceStates`(deviceRuntime.ts 的
// 纯 reducer)。这样聚合视图能一次读到全部设备,而迁移规则又能脱离 React 单测。

import { useEffect, useRef } from "react";
import type { TransportFactory } from "./App";
import type { PairedDevice } from "./devices";
import type { DeviceAction } from "./deviceRuntime";
import {
  connectDelayMs,
  shouldConnect,
  usagePollMs,
  type VisibilityState,
} from "./connectionPolicy";
import { reconcilePlan } from "./reconcilePlan";
import { loadCachedSessions, saveCachedSessions } from "./sessionCache";
import type { FleetTransport, RttSample } from "./transport";
import type {
  DecisionKind,
  DecisionRequest,
  PendingDecision,
  PendingSnapshot,
  SessionInfo,
  TodayUsage,
} from "./types";

/** 一台设备暴露给 UI 的操作面。存进 App 的 ref 表,供「答复这张卡」「下钻这条
 *  会话」这类需要点名某一台的动作使用。 */
export interface DeviceHandle {
  transport: FleetTransport;
  /** 主动拉一次权威快照(前台恢复、答复之后)。 */
  refresh: () => Promise<void>;
}

interface Props {
  device: PairedDevice;
  /** 会话快照缓存的命名空间。`null` = 用遗留全局键(同源形态:只有一个数据源);
   *  `undefined` = 完全不碰缓存(mock:固定数据,读缓存会把真数据画到假界面上)。 */
  storageId: string | null | undefined;
  makeTransport: TransportFactory;
  dispatch: (action: DeviceAction & { deviceId: string }) => void;
  /** 挂载/卸载时登记与注销这台的操作面。 */
  registerHandle: (deviceId: string, handle: DeviceHandle | null) => void;
  /** 这台此刻有没有待决策卡 —— 决定对账轮询的快慢档(reconcilePlan)。 */
  hasPendingDecisions: boolean;
  /** 那台桌面端在不在线 —— 两条轮询都以它为闸门。 */
  agentOnline: boolean;
  /** 这台是不是当前作用域那一台。只影响问得多勤(见 connectionPolicy.ts)。 */
  isActive: boolean;
  /** 这台在设备清单里的序号,用来错峰连接。 */
  index: number;
  /** 页面可见性。隐藏够久就把连接放掉 —— 后台通道是推送,不是这条 socket。 */
  visibility: VisibilityState;
}

/** `pending_snapshot` 的六类请求摊平成一串卡。 */
function flattenSnapshot(snap: PendingSnapshot, now: number): PendingDecision[] {
  const kinds: Array<[DecisionKind, DecisionRequest[] | undefined]> = [
    ["guard", snap.guard],
    ["elicitation", snap.elicitation],
    ["fleet-ask", snap.fleetAsk],
    ["plan-approval", snap.planApproval],
    ["permission-prompt", snap.permissionPrompt],
    ["a2ui-render", snap.a2uiRender],
  ];
  const out: PendingDecision[] = [];
  for (const [kind, list] of kinds) {
    for (const request of list ?? []) {
      out.push({ kind, id: request.id, request, arrivedAt: now });
    }
  }
  return out;
}

export function DeviceConnection({
  device,
  storageId,
  makeTransport,
  dispatch,
  registerHandle,
  hasPendingDecisions,
  agentOnline,
  isActive,
  index,
  visibility,
}: Props) {
  const deviceId = device.id;
  const clientRef = useRef<FleetTransport | null>(null);
  // 轮询 effect 里读的是当下的 dispatch/registerHandle，而它们来自 App 的
  // useCallback，本来就稳定；放进 ref 只是让下面的 effect 依赖表保持最小，
  // 免得一次无关的重渲把 socket 拆掉重连。
  const dispatchRef = useRef(dispatch);
  dispatchRef.current = dispatch;

  const refreshRef = useRef<() => Promise<void>>(async () => {});
  refreshRef.current = async () => {
    const client = clientRef.current;
    if (!client?.isAuthed) return;
    try {
      const snap = await client.request<PendingSnapshot>("pending_snapshot");
      dispatchRef.current({
        deviceId,
        type: "snapshot",
        fresh: flattenSnapshot(snap, Date.now()),
        agent: snap.agent,
        now: Date.now(),
      });
    } catch {
      // 桌面端离线 —— 实时事件之后会补上
    }
  };

  // 传输层的生命周期。依赖里除了「这台设备是哪一台、连哪个 relay」,只多一个
  // 可见性策略算出来的开关 —— 其余东西变了都不该把 socket 拆掉重连。
  const connectAllowed = shouldConnect(visibility, Date.now());
  useEffect(() => {
    if (!connectAllowed) return;
    const d = (a: DeviceAction) => dispatchRef.current({ ...a, deviceId });
    const client = makeTransport(device.secret, {
      onStatus: (connected) => d({ type: "status", connected }),
      onAgentOnline: (online) => {
        d({ type: "agentOnline", online });
        if (online) void refreshRef.current();
      },
      onDecisionCreated: (kind, request) =>
        d({
          type: "decisionCreated",
          kind,
          request: request as DecisionRequest,
          now: Date.now(),
        }),
      onDecisionResolved: (_kind, id) => d({ type: "decisionResolved", id }),
      onSessions: (list: SessionInfo[]) => {
        d({ type: "sessions", list });
        // 落盘那一份是给下次冷启动看的第一眼。全量与增量帧到这里时都已经并好
        // (见 relay.ts),所以缓存里始终是完整快照。
        if (storageId !== undefined) saveCachedSessions(storageId, list);
      },
      onSessionsKind: (kind) => d({ type: "sessionsKind", kind }),
      onRttSample: (sample: RttSample) => d({ type: "rtt", sample }),
      onReconnect: () => d({ type: "reconnect", now: Date.now() }),
      onAuthError: (message) => d({ type: "authError", message }),
    }, device.relayBase);
    clientRef.current = client;
    d({ type: "attach" });
    // 错峰:N 条连接同时握手会在网络恢复那一刻挤成一堆。
    const startAt = window.setTimeout(() => client.connect(), connectDelayMs(index));
    registerHandle(deviceId, { transport: client, refresh: () => refreshRef.current() });
    // 手机浏览器可能永远不跑 React 的清理函数(直接关标签),所以 pagehide 也说
    // 一声「我走了」,让桌面端不必等超时才把这台摘掉。
    const onPageHide = () => client.sayGoodbye();
    window.addEventListener("pagehide", onPageHide);
    return () => {
      window.clearTimeout(startAt);
      window.removeEventListener("pagehide", onPageHide);
      registerHandle(deviceId, null);
      clientRef.current = null;
      client.close();
    };
  }, [
    deviceId,
    storageId,
    device.secret,
    device.relayBase,
    makeTransport,
    registerHandle,
    connectAllowed,
    index,
  ]);

  // 冷启动先画缓存里的任务列表,免得 socket 还在握手时页面一片空白。
  useEffect(() => {
    if (storageId === undefined) return;
    let cancelled = false;
    void loadCachedSessions(storageId).then((cached) => {
      if (cancelled || !cached?.length) return;
      dispatchRef.current({ deviceId, type: "cachedSessions", list: cached });
    });
    return () => {
      cancelled = true;
    };
  }, [deviceId, storageId]);

  // 今日花费。桌面端在线时才轮询;数字由桌面端算好,手机只负责显示。
  useEffect(() => {
    if (!agentOnline) return;
    const intervalMs = usagePollMs(isActive);
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      const client = clientRef.current;
      // 页面不可见时跳过这一轮(但仍排下一次):后台问一个只用来显示的数字,
      // 纯属白烧电与流量。
      if (client?.isAuthed && document.visibilityState === "visible") {
        try {
          const usage = await client.request<TodayUsage>("today_usage");
          if (!cancelled) dispatchRef.current({ deviceId, type: "usage", usage });
        } catch {
          /* 瞬时失败 —— 保留上一次的值 */
        }
      }
      if (!cancelled) timer = window.setTimeout(poll, intervalMs);
    };
    void poll();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentOnline, deviceId, isActive]);

  // 待决策卡的前台兜底对账。decision_created / decision_resolved 都是无 ack 无
  // 重传的广播,弱网掉一帧就会漏一张卡;(重)连时那一次 refresh 也可能超时被吞。
  // 这条循环是耐用的兜底:有卡时快(3s)、闲时慢(15s),节奏由 reconcilePlan 决定。
  useEffect(() => {
    const plan = reconcilePlan(agentOnline, hasPendingDecisions);
    if (!plan.poll) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      if (document.visibilityState === "visible") await refreshRef.current();
      if (!cancelled) timer = window.setTimeout(tick, plan.intervalMs);
    };
    timer = window.setTimeout(tick, plan.intervalMs);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentOnline, hasPendingDecisions]);

  // 回到前台时补拉一次:手机浏览器会冻结后台标签的 socket,重连虽然会发生,但
  // 那一刻的快照可能已经过期。
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") void refreshRef.current();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, []);

  return null;
}
