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
import { SUPPORTS_PUSH } from "./hostMode";
import { enablePush } from "./push";
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
  /** 这台此刻连上中转了没有。推送订阅是直接写 socket 的(无队列无重传),所以
   *  必须等它为真才注册 —— 否则那一帧掉在地上,relay 那边永远不会有这条订阅。 */
  connected: boolean;
  /** 浏览器/系统层面的通知权限是否已授予(整部手机一份)。 */
  pushGranted: boolean;
  /** 用户是否把**这一台**的通知关掉了。 */
  pushMuted: boolean;
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
  connected,
  pushGranted,
  pushMuted,
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
    // 不再要求 `isAuthed`。它说的是「那条流/socket 此刻是开的」,而这次请求走的是
    // 另一条通道 —— 同源形态下是普通 fetch,流断了主机往往仍然答得动。拿它当闸门
    // 会让唯一一条能捞回漏掉卡片的路,恰好在最需要它的时候关上(见 reconcilePlan)。
    if (!client) return;
    try {
      const snap = await client.request<PendingSnapshot>("pending_snapshot");
      dispatchRef.current({
        deviceId,
        type: "snapshot",
        fresh: flattenSnapshot(snap, Date.now()),
        agent: snap.agent,
        now: Date.now(),
      });
      // 主机真答了这一次,那它就是在线的。反过来不成立:单次请求失败不足以判死,
      // 「离线」这个信号仍然由流/socket 自己拥有。
      dispatchRef.current({ deviceId, type: "agentOnline", online: true });
    } catch {
      // 桌面端离线 —— 实时事件或下一轮探测之后会补上
    }
  };

  // 传输层的生命周期。依赖里除了「这台设备是哪一台、连哪个 relay」,只多一个
  // 可见性策略算出来的开关 —— 其余东西变了都不该把 socket 拆掉重连。
  const connectAllowed = shouldConnect(visibility, Date.now());
  useEffect(() => {
    if (!connectAllowed) return;
    const d = (a: DeviceAction) => dispatchRef.current({ ...a, deviceId });
    const client = makeTransport(device, {
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
    });
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
    // 依赖里放 `device` 整体:换密钥 / 换 relay / 换 baseUrl / 换 token 都该重连,
    // 而 devices.ts 的每一次修改都产出新对象(纯函数层),所以引用比较正好等价于
    // 「这台设备的连接参数变了没有」。
  }, [
    deviceId,
    storageId,
    device,
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
  // 这条循环是耐用的兜底:有卡时快(3s)、闲时慢(15s)、看着离线时更慢(30s),节奏
  // 由 reconcilePlan 决定 —— 离线那一档正是老板那台服务器上「只能刷新页面」的解药。
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

  // 推送订阅按**设备**登记:relay 的订阅是每个 channel 一份文件,所以一部手机
  // 想收到 N 台的通知,就得在 N 个 channel 下各注册一次(同一个浏览器 endpoint)。
  //
  // `connected` 是依赖而不是顺手:pushSubscribe 直接写 socket,socket 没 OPEN 时
  // 返回 false 且不排队不重传。历史上这个 effect 只依赖 [push, optedOut],注册
  // 因此总在握手之前发生 —— 手机报告自己已订阅,而 relay 的订阅库里什么都没有。
  //
  // enablePush 是幂等的:已有订阅就复用,权限已授予时 requestPermission 直接返回。
  useEffect(() => {
    if (!SUPPORTS_PUSH) return;
    if (!connected || !pushGranted || pushMuted) return;
    // HTTP 直连那条传输层没有推送通道(pushSubscribe 恒返回 false),订阅也就无处
    // 登记 —— 别去问 VAPID 公钥,那个请求只会白打一次。
    if (device.kind !== "relay") return;
    const client = clientRef.current;
    if (!client) return;
    void enablePush(client, device.relayBase, deviceId);
  }, [connected, pushGranted, pushMuted, deviceId, device]);

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
