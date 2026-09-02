import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { BookOpen, Inbox, ListChecks, MoreHorizontal, Plus } from "lucide-react";
import styles from "./App.module.css";
import {
  disablePush,
  enablePush,
  isPushMuted,
  setPushMuted as setPushMutedStored,
  setPushOptedOut,
  pushState,
  unsubscribeChannel,
  type PushState,
} from "./push";
// 这里不再认识任何一个具体传输层。造哪一个由 main.tsx 按构建模式决定并注入
// —— 那处选择是一对动态 import，同源构建把 relay 那条分支连同整棵依赖树一起
// 消掉，而如果 App 静态 import 了 RelayClient，那套安排就白做了。
import type { FleetTransport, TransportHandlers } from "./transport";
import { NEEDS_PAIRING, SUPPORTS_PUSH } from "./hostMode";
import { formatRttSplit } from "./connQuality";
import { DeviceConnection, type DeviceHandle } from "./DeviceConnection";
import { HIDDEN_DISCONNECT_MS, type VisibilityState } from "./connectionPolicy";
import {
  aggregateDecisions,
  aggregateSessions,
  allDecisionsLoaded,
  anyAgentOnline,
  anyConnected,
  anySessionsLoaded,
  itemKey,
  devicesReducer,
  emptyDeviceState,
  totalUsage,
  worstCongestion,
  type DeviceStates,
  type WithDevice,
} from "./deviceRuntime";
// 只取零依赖的开关。`?mock` 那个假客户端 extends RelayClient，从这里 import
// 会把整棵 relay 依赖树静态拖进同源构建 —— 造它的活儿归 transportRelay.ts。
import { isMockMode } from "./mockMode";
import type { RepoSummary, SessionInfo, WikiDoc } from "./types";
import { randomId } from "./clientId";
import { needsA2hsForDurableStorage } from "./secretStore";
import {
  activeDevice,
  addPendingUnsub,
  adoptScannedDevice,
  clearBook,
  consumeHashSecret,
  dropPendingUnsub,
  emptyBook,
  loadBookFromIdb,
  loadBookSync,
  loadPendingUnsub,
  nextDeviceLabel,
  persistBook,
  removeDevice,
  renameDevice,
  setActiveDevice,
  type DeviceBook,
  type PairedDevice,
} from "./devices";
import { onPairingLink } from "./deepLink";
import { clearCachedSessions } from "./sessionCache";
import { AUTH_WAIT_MS, waitAuthed } from "./transportWait";
import { DeviceScopeProvider, scopedKey } from "./deviceScope";
import { t as translate, useI18n } from "./i18n";
import { ExitGuard, installUnloadPrompt } from "./exitGuard";
import { HistoryLayer, setRootBackHandler } from "./useNavStack";
import { NEW_SESSION_DRAFT_KEY, NewSessionSheet } from "./views/Composer";
import { clearDraftsByPrefix, loadDraft, saveDraft } from "./draft";
import { onShareReceived, shareToPrompt, sharedFilesToFiles } from "./shareTarget";
import { onNativePushToken } from "./nativePush";
import { onDecisionDeepLink } from "./decisionDeepLink";
import { DecisionsView } from "./views/DecisionsView";
import { DecisionDrawer } from "./views/DecisionDrawer";
import { MoreView } from "./views/MoreView";
import { PlansView } from "./views/PlansView";
import { ArtifactsView } from "./views/ArtifactsView";
import { RepoView } from "./views/RepoView";
import { RepoDetailView } from "./views/RepoDetailView";
import { SessionDetailView } from "./views/SessionDetailView";
import { TasksView } from "./views/TasksView";
import { UsageView } from "./views/UsageView";
import { WikiView } from "./views/WikiView";
import { WikiDocView } from "./views/WikiDocView";

const A2HS_DISMISSED_KEY = "fleet-a2hs-dismissed";

/** Compact token count: 1.2M / 34.5K / 780. */
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return `${n}`;
}

type Tab = "decisions" | "tasks" | "wiki" | "more";

/** 一台新配对设备的默认字段。默认名走 i18n，所以它在这里而不在 devices.ts ——
 *  那一层刻意不认识 i18n，好让它整层保持可测的纯函数。 */
function newDeviceMint(book: DeviceBook): { id: string; label: string; now: number } {
  return { id: randomId(), label: nextDeviceLabel(book, translate("设备")), now: Date.now() };
}

// `?mock` runs the whole app on fixtures (no relay, no pairing) — used by the
// promo screen-recording pipeline and for quick UI work in a plain browser.
const MOCK = isMockMode();

/** 造出这次部署该用的传输层。由 main.tsx 按构建模式注入。 */
export type TransportFactory = (
  secret: string,
  handlers: TransportHandlers,
  /** 这台设备指名的 relay;`null` = 构建默认值。同源形态忽略它。 */
  relayBase?: string | null,
) => FleetTransport;

/** mock / 同源形态没有配对这回事,但下游一切都以「一台设备」为单位。给它们各
 *  合成一台,整条链路就只有一种形状 —— 不必在每个视图里再分一次叉。 */
const MOCK_DEVICE: PairedDevice = {
  id: "mock",
  label: "Mock",
  secret: "mock-secret",
  relayBase: null,
  addedAt: 0,
};
const SAME_ORIGIN_DEVICE: PairedDevice = {
  id: "same-origin",
  label: "",
  secret: "same-origin",
  relayBase: null,
  addedAt: 0,
};

/** 还没收到过任何一帧的设备读到的状态。模块级常量:每次渲染新建一个对象会让
 *  下面那些解构出来的数组每渲染都换引用。 */
const EMPTY_DEVICE_STATE = emptyDeviceState();

export function App({ makeTransport }: { makeTransport: TransportFactory }) {
  // 订阅语言切换：App 根重渲即可带动整树（无 React.memo），各处 t() 现算。
  const { t } = useI18n();
  // 这台手机配对过的每一台 Fleet（devices.ts）。`?mock` 与同源形态都没有配对
  // 这回事，簿子留空，下面的 `secret` 直接给占位串。
  const [book, setBook] = useState<DeviceBook>(() => {
    if (MOCK || !NEEDS_PAIRING) return emptyBook();
    const stored = loadBookSync(newDeviceMint(emptyBook()));
    // PWA 是被一个带 `#k=…` 的 URL 打开的 —— 那就是一次配对。
    const scanned = consumeHashSecret();
    if (!scanned) return stored;
    return adoptScannedDevice(stored, scanned.secret, newDeviceMint(stored), scanned.relayBase, {
      // 壳的启动重注不抢焦点:用户切过去的那一台不该每次重开 app 就被打回原形。
      focus: !scanned.boot,
    }).book;
  });
  // 当前作用域设备的密钥。`?mock` stands in for a pairing secret so the gate
  // below opens and the effect that builds the client runs — it just builds a
  // MockRelayClient. 同源形态没有配对这回事（后端就是发出这张页面的那个进程），
  // 所以那道门整个不存在，直接给一个占位串让下面建连接的 effect 跑起来。
  const current = NEEDS_PAIRING && !MOCK ? activeDevice(book) : null;
  const secret = MOCK
    ? "mock-secret"
    : NEEDS_PAIRING
      ? (current?.secret ?? null)
      : "same-origin";
  // 当前设备指名的 relay。`null` = 构建默认值（迁移过来的设备、以及扫码时没带
  // `&relay=` 的都是这一类），传输层与推送各自把它解析成实际地址。
  const relayBase = current?.relayBase ?? null;
  // 本地持久化的命名空间。会话快照缓存、草稿、附件、workspace 记忆都按它分家
  // ——那些内容只对某一台机器有意义（见 deviceScope.tsx）。mock 与同源形态不分家
  // （它们只有一个数据源，加前缀只会让老用户已有的草稿凭空消失）。
  const deviceId = current?.id ?? null;

  // 运行时视角的设备清单：配对形态是簿子本身，mock / 同源各是一台合成设备。
  const runtimeDevices = useMemo(
    () => (MOCK ? [MOCK_DEVICE] : NEEDS_PAIRING ? book.devices : [SAME_ORIGIN_DEVICE]),
    [book.devices],
  );
  const deviceOrder = useMemo(() => runtimeDevices.map((d) => d.id), [runtimeDevices]);
  /** 当前作用域那一台的运行时 id。 */
  const activeDeviceId =
    MOCK || !NEEDS_PAIRING ? runtimeDevices[0].id : (deviceId ?? "");
  // 分享菜单那条 effect 的依赖是空的（只在挂载时订阅一次），但它开火时要用**当下**
  // 的设备，所以那一处读 ref 而不是闭包里的值。
  const deviceIdRef = useRef<string | null>(deviceId);
  deviceIdRef.current = deviceId;
  // 同理：清除全部配对那个回调的依赖是空的，但它要遍历**当下**的簿子。
  const bookRef = useRef(book);
  bookRef.current = book;
  // null = still probing IndexedDB; only after that fails do we show the gate.
  const [idbProbed, setIdbProbed] = useState(false);
  const [a2hsDismissed, setA2hsDismissed] = useState(
    () => localStorage.getItem(A2HS_DISMISSED_KEY) === "1",
  );

  // localStorage wiped (iOS 7-day eviction, cache clear) but the IDB copy may
  // have survived — re-hydrate before declaring the pairing lost.
  useEffect(() => {
    if (secret) {
      setIdbProbed(true);
      return;
    }
    let cancelled = false;
    loadBookFromIdb(newDeviceMint(emptyBook())).then((recovered) => {
      if (cancelled) return;
      if (recovered) {
        // 把活下来的那一份写回两个存储，下次冷启动就不必再走兜底。
        persistBook(recovered);
        setBook(recovered);
      }
      setIdbProbed(true);
    });
    return () => {
      cancelled = true;
    };
  }, [secret]);

  // Native shell only: the app boots from bundled assets, so there is no URL to
  // read the pairing secret from. Universal Links / App Links deliver the
  // scanned pairing URL here instead. No-op in the browser/PWA.
  useEffect(() => {
    return onPairingLink((paired) => {
      // 与 PWA 的 `#k=` 路径走同一个入口：去重、保留用户改过的名字、焦点落到
      // 刚扫的那台，三条规则只有一份实现（devices.ts::adoptScannedDevice）。
      setBook((prev) => adoptScannedDevice(prev, paired, newDeviceMint(prev)).book);
      setIdbProbed(true);
    });
  }, []);

  // ── 设备管理（「更多」页那一块）─────────────────────────────────────────
  //
  // 三个动作都是「改簿子 + 落盘」，落盘紧跟状态更新，免得一次意外退出让用户
  // 以为改过的东西没了。

  /** 清除全部配对。每台设备的退订都先记进待办（而不是在这里逐台连上去退）：
   *  紧接着就要 reload，等不了 N 条临时连接的握手；下次启动那条重试 effect 会
   *  把它们补掉。 */
  const unpairAll = useCallback(() => {
    const now = Date.now();
    for (const d of bookRef.current.devices) {
      if (SUPPORTS_PUSH) addPendingUnsub({ secret: d.secret, relayBase: d.relayBase, at: now });
      clearCachedSessions(d.id);
      clearDraftsByPrefix(scopedKey(d.id, ""));
    }
    clearBook();
    location.reload();
  }, []);

  const switchDevice = useCallback((id: string) => {
    setBook((prev) => {
      const next = setActiveDevice(prev, id);
      persistBook(next);
      return next;
    });
  }, []);

  const renameDeviceLabel = useCallback((id: string, label: string) => {
    setBook((prev) => {
      const next = renameDevice(prev, id, label);
      persistBook(next);
      return next;
    });
  }, []);

  /** 让某台设备的 relay channel 停止推送。
   *
   *  它可能不是当前连着的那一台，所以为它临时开一条连接。不能靠「本地退订」
   *  代替：浏览器订阅是所有设备共用的一份，退掉它等于把其他设备的通知一起
   *  掐掉（见 push.ts::unsubscribeChannel）。
   *
   *  返回是否退成。退不成不该拦住用户移除设备——那是他明确的意图——所以调用方
   *  把它记进待办退订，下次启动重试。 */
  const stopPushFor = useCallback(
    async (device: PairedDevice): Promise<boolean> => {
      // 这台设备正连着的话直接借用它那条连接。
      const live = handlesRef.current[device.id]?.transport;
      if (live?.isAuthed) return unsubscribeChannel(live);
      const temp = makeTransport(device.secret, {}, device.relayBase);
      try {
        temp.connect();
        if (!(await waitAuthed(temp, AUTH_WAIT_MS))) return false;
        return await unsubscribeChannel(temp);
      } catch {
        return false;
      } finally {
        temp.close();
      }
    },
    [makeTransport],
  );

  const removeDeviceEntry = useCallback(
    async (device: PairedDevice) => {
      // 先退订、再改簿子：反过来的话失败重试就没有 secret 可用了（记待办那条
      // 路仍然需要它）。
      const unsubscribed = SUPPORTS_PUSH ? await stopPushFor(device) : true;
      if (!unsubscribed) {
        addPendingUnsub({ secret: device.secret, relayBase: device.relayBase, at: Date.now() });
      }
      // 这台设备的本地痕迹一并清掉：会话快照缓存 + 它命名空间下的全部草稿
      // （新会话表单、附件路径、上次用的 repo、各会话的半截输入）。
      clearCachedSessions(device.id);
      clearDraftsByPrefix(scopedKey(device.id, ""));
      setBook((prev) => {
        const next = removeDevice(prev, device.id);
        persistBook(next);
        return next;
      });
    },
    [stopPushFor],
  );

  /** 上次没退成的退订，启动后补一次。失败就留着，等下次（7 天后过期，见
   *  devices.ts）。 */
  useEffect(() => {
    if (!SUPPORTS_PUSH || MOCK) return;
    const pending = loadPendingUnsub(Date.now());
    if (pending.length === 0) return;
    let cancelled = false;
    void (async () => {
      for (const entry of pending) {
        if (cancelled) return;
        const temp = makeTransport(entry.secret, {}, entry.relayBase);
        try {
          temp.connect();
          if (await waitAuthed(temp, AUTH_WAIT_MS)) {
            if (await unsubscribeChannel(temp)) dropPendingUnsub(entry.secret, Date.now());
          }
        } catch {
          // 留着下次再试
        } finally {
          temp.close();
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [makeTransport]);

  // Android share sheet → new-session composer. Seed the draft *before* opening
  // the sheet: NewSessionSheet reads it once on mount via useDraft, so writing
  // after would be ignored. Merging (rather than replacing) keeps whatever
  // workspace/model the user last picked.
  useEffect(() => {
    return onShareReceived((share) => {
      void (async () => {
        const files = await sharedFilesToFiles(share.files);
        // Files that became real attachments don't need naming in the prose —
        // the chips already show them. Only the ones we failed to read stay in
        // the text, so the user still knows something came along.
        const fetched = new Set(files.map((f) => f.name));
        const missed = share.files.filter((f) => !fetched.has(f.name));
        const prompt = shareToPrompt({ ...share, files: missed });
        // 草稿按设备分家，否则从分享菜单塞进来的提示词会落进另一台的新会话表单。
        const key = scopedKey(deviceIdRef.current, NEW_SESSION_DRAFT_KEY);
        const existing = loadDraft<Record<string, unknown>>(key, {});
        saveDraft(key, { ...existing, prompt });
        setSharedFiles(files);
        setShowNewSession(true);
      })();
    });
  }, []);

  const [tab, setTab] = useState<Tab>("decisions");
  /// 通知点击要聚焦的决策卡。nonce 让「同一张卡被连点两次」也能触发;deviceId
  /// 是从通知里的来源标记反查出来的(老 relay 不盖标记时为 undefined)。
  const [focusDecision, setFocusDecision] = useState<{
    id: string;
    deviceId?: string;
    nonce: number;
  } | null>(null);

  // ── 每台设备的运行时 ─────────────────────────────────────────────────────
  //
  // 从前这里是一把扁平的 useState:一个 connected、一个 sessions、一个 decisions。
  // 那把状态把「只有一个数据源」焊死进了组件。现在每台设备一份(deviceRuntime.ts
  // 的纯 reducer),连接则由每台一个 <DeviceConnection> 负责。
  const [states, dispatch] = useReducer(devicesReducer, {} as DeviceStates);
  // 每台设备的操作面(transport + 主动刷新)。用 state 而不是 ref:UI 要在连接
  // 建立那一刻重新渲染,才能把 transport 交给下面各个视图。
  const [handles, setHandles] = useState<Record<string, DeviceHandle>>({});
  const registerHandle = useCallback((id: string, handle: DeviceHandle | null) => {
    setHandles((prev) => {
      if (!handle) {
        if (!(id in prev)) return prev;
        const next = { ...prev };
        delete next[id];
        return next;
      }
      return { ...prev, [id]: handle };
    });
  }, []);

  // stopPushFor / unpairAll 这类空依赖的回调要读**当下**的连接表。
  const handlesRef = useRef(handles);
  handlesRef.current = handles;

  /** 某一台设备的传输层。合并列表里点进去的东西属于**那一台**,不是当前作用域
   *  那一台 —— 拿错了就是一次打错地方的请求。 */
  const transportFor = useCallback(
    (id: string): FleetTransport | null => handles[id]?.transport ?? null,
    [handles],
  );

  const activeState = states[activeDeviceId] ?? EMPTY_DEVICE_STATE;
  const client = handles[activeDeviceId]?.transport ?? null;
  const {
    sessionsFrame,
    rttSplit,
    snapshotSources,
    authError,
  } = activeState;
  // 首屏「正在加载任务」的闸门:有任意一台推过首帧就不再算加载中。
  const sessionsLoaded = anySessionsLoaded(states, deviceOrder);
  // 决策卡是**合并**的:全部设备的卡按到达时间排在一起,每张带着归属设备。
  // 这就是多设备的核心价值——一个收件箱,而不是「记得去另一台看看」。
  const decisions = useMemo(
    () => aggregateDecisions(states, deviceOrder),
    [states, deviceOrder],
  );
  // 骨架屏只在**每一台**都还没回过首份快照时显示;有一台回了就先画它的卡。
  const decisionsLoaded = allDecisionsLoaded(states, deviceOrder);
  /** 卡片/列表上的设备徽标。只配了一台时返回 null —— 单设备用户不该为多设备
   *  付出一行视觉噪音。 */
  const deviceLabelOf = useCallback(
    (id: string): string | null => {
      if (runtimeDevices.length <= 1) return null;
      return runtimeDevices.find((d) => d.id === id)?.label ?? null;
    },
    [runtimeDevices],
  );
  // 头部那三样看的是**全体**:一台离线不该让整个界面显示离线,而用户感觉到的
  // 拥塞是最卡的那条链路。花费是所有设备当日之和。
  const connected = anyConnected(states, deviceOrder);
  const agentOnline = anyAgentOnline(states, deviceOrder);
  const congestion = worstCongestion(states, deviceOrder);
  const todayUsage = totalUsage(states, deviceOrder);

  const [push, setPush] = useState<PushState>(pushState);
  // Sub-state below "granted": the user can turn notifications off even while
  // the browser permission stays granted. Persisted so it survives reloads.
  // 静音按设备各一份。总开关看的是「是不是每一台都被关掉」——只关了一台时
  // 横幅不该说「通知已关闭」,那会让人以为另一台也不响了。
  const [pushMuted, setPushMuted] = useState<Record<string, boolean>>({});
  useEffect(() => {
    setPushMuted((prev) => {
      let changed = false;
      const next = { ...prev };
      for (const d of runtimeDevices) {
        if (!(d.id in next)) {
          next[d.id] = isPushMuted(d.id);
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [runtimeDevices]);
  const pushOptedOut =
    runtimeDevices.length > 0 && runtimeDevices.every((d) => pushMuted[d.id] === true);
  // 会话详情是一条下钻链而不是单页：从 Agent 卡「打开子代理」往上叠一层子代理会话，
  // 返回逐层退回（和知识库 wikiStack 同一套栈式浮层模型）。栈顶 id 解析出当前详情。
  // 每一层都带上归属设备:会话 id 只在单机内唯一,而下钻要用**那一台**的
  // transport 去拉 tail(见 itemKey 的说明)。
  const [detailStack, setDetailStack] = useState<Array<{ deviceId: string; id: string }>>([]);
  // 知识库文档是一条链而不是单页：`[[slug]]` 站内跳转往上叠一篇，返回逐篇退回。
  const [wikiStack, setWikiStack] = useState<Array<{ deviceId: string; doc: WikiDoc }>>([]);
  const [showRepo, setShowRepo] = useState(false);
  const [repoDetail, setRepoDetail] = useState<{ deviceId: string; repo: RepoSummary } | null>(
    null,
  );
  const [showUsage, setShowUsage] = useState(false);
  const [showPlans, setShowPlans] = useState(false);
  const [showArtifacts, setShowArtifacts] = useState(false);
  const [showNewSession, setShowNewSession] = useState(false);
  // Files handed over by another app's share, pending upload once the
  // new-session sheet mounts (that's where the attachment state lives).
  const [sharedFiles, setSharedFiles] = useState<File[]>([]);
  const [exitArmed, setExitArmed] = useState(false);

  // 栈底（主页 tab、无浮层）按返回：先拦一次给「再按一次退出」，再按才真走。
  // beforeunload 只覆盖刷新/关标签/地址栏跳走这些非返回路径——mock 模式不装，
  // 免得录屏流水线被原生对话框卡住。
  useEffect(() => {
    const install = () => (MOCK ? () => {} : installUnloadPrompt());
    let uninstall = install();
    let timer: number | undefined;
    const guard = new ExitGuard(setExitArmed, () => {
      uninstall();
      // 万一没走成（本页就是历史里的第一条，退无可退），把兜底装回来。
      timer = window.setTimeout(() => {
        uninstall = install();
      }, 1_000);
    });
    setRootBackHandler(guard.handleRootBack);
    return () => {
      if (timer !== undefined) window.clearTimeout(timer);
      uninstall();
      setRootBackHandler(() => "leave");
    };
  }, []);

  /** 本机刚答完一张卡:先乐观移除,并记下时间戳,压住迟到快照把它复活
   *  (deviceRuntime.ts 的 answered/snapshot 两个 action)。答复发给哪一台由
   *  调用方点名 —— 收件箱是合并的,卡不一定属于当前作用域那台。 */
  const markAnswered = useCallback((deviceId: string, id: string) => {
    dispatch({ deviceId, type: "answered", id, now: Date.now() });
  }, []);

  // 页面可见性。多设备之后它是**连接策略**的输入而不只是一次补拉的触发:隐藏
  // 够久就把 N 条 socket 全放掉(后台通道是推送,不是这条 socket),回到前台再
  // 错峰重连。见 connectionPolicy.ts。
  const [visibility, setVisibility] = useState<VisibilityState>(() => ({
    visible: typeof document === "undefined" || document.visibilityState === "visible",
    hiddenSince: Date.now(),
  }));
  useEffect(() => {
    const onChange = () => {
      const visible = document.visibilityState === "visible";
      setVisibility((prev) =>
        prev.visible === visible ? prev : { visible, hiddenSince: Date.now() },
      );
    };
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);
  // 隐藏之后到了宽限期就重算一次(否则「该断开了」这件事没有任何东西来触发)。
  useEffect(() => {
    if (visibility.visible) return;
    const left = HIDDEN_DISCONNECT_MS - (Date.now() - visibility.hiddenSince);
    const timer = window.setTimeout(
      () => setVisibility((prev) => ({ ...prev })),
      Math.max(left, 0) + 100,
    );
    return () => window.clearTimeout(timer);
  }, [visibility]);

  // 回到前台/重新获得焦点时重算推送状态:Notification.permission 可能在后台期间
  // 被改过(老板在系统设置里开了或关了),而横幅只在挂载时读过一次 pushState()。
  // 快照的补拉不在这里 —— 那是每台设备自己的事(DeviceConnection)。
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      setPush(pushState());
    };
    const onFocus = () => setPush(pushState());
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", onFocus);
    // The Permissions API fires 'change' the instant the setting flips, even
    // without a focus/visibility bounce. Best-effort: older Safari lacks it.
    let perm: PermissionStatus | undefined;
    navigator.permissions
      ?.query({ name: "notifications" as PermissionName })
      .then((status) => {
        perm = status;
        status.addEventListener("change", onFocus);
      })
      .catch(() => {
        /* Permissions API / "notifications" name unsupported; focus+visibility cover it */
      });
    return () => {
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", onFocus);
      perm?.removeEventListener("change", onFocus);
    };
  }, []);

  // 订阅本身不在这里了:它按**设备**登记(relay 的订阅是每个 channel 一份文件),
  // 所以那段逻辑住在 DeviceConnection —— 每台设备各自在自己连上之后注册一次。

  // 原生壳交来厂商推送 token。到达时机不定（壳要先过系统通知授权），所以只
  // 重算一次 push 状态 —— classifyPush 见到 token 就返回 granted，随后那个
  // "granted 且未 opt-out 就 enablePush" 的 effect 会把它注册到 relay。注册
  // 逻辑因此只有一份，不必在这里重复一遍。
  useEffect(() => {
    return onNativePushToken(() => setPush(pushState()));
  }, []);

  // 点通知 → 直达那张决策卡。三条投递路径(冷启动 URL / SW 前台补发 /
  // 原生壳注入)都汇进 onDecisionDeepLink,这里只管拿到目标后切页+聚焦。
  //
  // 用 nonce 而不是只存 id:连点同一条通知时 id 不变,单靠 id 的话 DecisionsView
  // 那边的 effect 不会重跑,表现为「第二次点没反应」。
  useEffect(() => {
    return onDecisionDeepLink((target) => {
      setTab("decisions");
      setFocusDecision({ id: target.id, nonce: Date.now() });
      // relay 盖的来源标记是 channel id 的前缀;手机手里只有配对 secret,所以要
      // 现算一遍每台设备的 channel id 去比对。异步(SubtleCrypto),所以先按 id
      // 聚焦、拿到设备后再精确到那一台 —— 点击不必等一次哈希。
      //
      // 动态 import + 直接写 define 的原表达式:relayCrypto 属于 relay 侧,而
      // 同源(webui)产物里不许有它。经 const 中转 Rollup 就不折叠了(见
      // main.tsx 与 hostMode.test.ts 记的那两次实测)。
      const mark = target.channelMark;
      if (!mark || import.meta.env.VITE_FLEET_HOST === "webui") return;
      void (async () => {
        const { channelIdOf } = await import("./relayCrypto");
        for (const d of bookRef.current.devices) {
          const id = await channelIdOf(d.secret);
          if (!id.startsWith(mark)) continue;
          setFocusDecision({ id: target.id, deviceId: d.id, nonce: Date.now() });
          return;
        }
      })();
    });
  }, []);

  /** 总开关「开」:先用当前设备那条连接把系统权限问下来并注册,再把其余设备的
   *  静音位一起清掉 —— 它们各自的 effect 会接着注册自己那份。 */
  const handleEnablePush = useCallback(async () => {
    const ids = runtimeDevices.map((d) => d.id);
    if (client) setPush(await enablePush(client, relayBase, activeDeviceId));
    setPushOptedOut(false, ids);
    setPushMuted(Object.fromEntries(ids.map((id) => [id, false])));
  }, [client, relayBase, activeDeviceId, runtimeDevices]);

  /** 总开关「关」:每一台都要退订。只退当前那台的话,其余几台会继续推 —— 而
   *  用户刚刚明确表示「别再响了」。 */
  const handleDisablePush = useCallback(async () => {
    const ids = runtimeDevices.map((d) => d.id);
    setPushOptedOut(true, ids);
    setPushMuted(Object.fromEntries(ids.map((id) => [id, true])));
    for (const id of ids) {
      const t = handlesRef.current[id]?.transport;
      if (!t) continue;
      // 当前那台走 disablePush(它还会撤掉浏览器订阅本体);其余只发退订帧 ——
      // 浏览器订阅是所有设备共用的一份,撤了就等于把还没关的那些也一起掐掉。
      if (id === activeDeviceId) await disablePush(t, id);
      else await unsubscribeChannel(t);
    }
  }, [runtimeDevices, activeDeviceId]);

  /** 只关/只开某一台(设备列表里的那个开关)。这里绝不碰浏览器订阅本体。 */
  const handleMuteDevice = useCallback(
    async (device: PairedDevice, muted: boolean) => {
      setPushMuted((prev) => ({ ...prev, [device.id]: muted }));
      setPushMutedStored(device.id, muted);
      const t = handlesRef.current[device.id]?.transport;
      if (!t) return;
      if (muted) await unsubscribeChannel(t);
      else await enablePush(t, device.relayBase, device.id);
    },
    [],
  );

  // Optimistic read stamps: the server re-derives lastReadMs on its next scan,
  // so until that push arrives we overlay the local click/dwell time.
  const [localReadMs, setLocalReadMs] = useState<Record<string, number>>({});
  const markRead = useCallback(
    (items: Array<WithDevice<SessionInfo>>) => {
      if (items.length === 0) return;
      // 一批里可能混着不同设备的会话(合并列表里「全部标记已读」),所以按设备
      // 分组各发一次 —— 发错设备的话对方根本不认识这些 id。
      const byDevice = new Map<string, Array<WithDevice<SessionInfo>>>();
      for (const s of items) {
        const list = byDevice.get(s.deviceId) ?? [];
        list.push(s);
        byDevice.set(s.deviceId, list);
      }
      for (const [id, list] of byDevice) {
        handlesRef.current[id]?.transport
          .request("session_read", {
            items: list.map((s) => ({ sessionId: s.id, workspacePath: s.workspacePath })),
          })
          .catch(() => {});
      }
      const now = Date.now();
      setLocalReadMs((prev) => {
        const next = { ...prev };
        for (const s of items) next[itemKey(s.deviceId, s.id)] = now;
        return next;
      });
    },
    [],
  );

  // 任务列表也是**合并**的:全部设备的会话排在一起,每条带着归属设备。本地那份
  // 乐观已读时间戳按复合键覆盖上去(服务端下次扫描会重新算出 lastReadMs)。
  const mergedSessions = useMemo<Array<WithDevice<SessionInfo>>>(
    () =>
      aggregateSessions(states, deviceOrder)
        .map((s) => {
          const local = localReadMs[itemKey(s.deviceId, s.id)];
          return local && local > (s.lastReadMs ?? 0) ? { ...s, lastReadMs: local } : s;
        })
        .sort((a, b) => (b.lastActivityMs ?? 0) - (a.lastActivityMs ?? 0)),
    [states, deviceOrder, localReadMs],
  );

  /** 当前作用域那一台的会话。按设备作用域的页面(新会话、计划、会话详情里的
   *  子代理解析)用它 —— 那些页面问的是「这一台上有什么」,把别台的混进去只会
   *  让它们看到自己拉不动的 id。 */
  const scopedSessions = useMemo(
    () => mergedSessions.filter((s) => s.deviceId === activeDeviceId),
    [mergedSessions, activeDeviceId],
  );

  // 决策卡上「这张卡属于哪个会话」的反查。键是复合键 —— 两台机器上同号的会话
  // 是两条不同的会话。
  const workspaceOf = useMemo(() => {
    const map = new Map<string, WithDevice<SessionInfo>>();
    for (const s of mergedSessions) map.set(itemKey(s.deviceId, s.id), s);
    return (deviceId: string, sessionId: string) => map.get(itemKey(deviceId, sessionId));
  }, [mergedSessions]);

  // The detail page re-derives its session from the live snapshot so status /
  // plan chips stay fresh while it is open. Top of the drill-down stack wins.
  const detailSession = useMemo(() => {
    const top = detailStack[detailStack.length - 1];
    if (!top) return null;
    return (
      mergedSessions.find((s) => s.deviceId === top.deviceId && s.id === top.id) ?? null
    );
  }, [mergedSessions, detailStack]);

  // Open a session as a fresh root detail (from the tasks / decisions lists).
  const openSessionRoot = useCallback(
    (deviceId: string, id: string) => setDetailStack([{ deviceId, id }]),
    [],
  );
  // Drill into a subagent from within a detail view — push another layer so the
  // back button returns to the opener. `AgentNav.open` prefixes `agent-`.
  const openSessionById = useCallback(
    (deviceId: string, id: string) => setDetailStack((s) => [...s, { deviceId, id }]),
    [],
  );

  // A secondary page (session detail / wiki doc / repo / usage / new session)
  // is covering the tab body. Used to decide whether the global decision drawer
  // should take over as the answering surface.
  const overlayOpen =
    detailStack.length > 0 ||
    wikiStack.length > 0 ||
    showRepo ||
    repoDetail !== null ||
    showUsage ||
    showPlans ||
    showArtifacts ||
    showNewSession;
  // Float the decision drawer over whatever the boss is looking at — EXCEPT the
  // plain 决策 tab, which already renders the cards inline (no overlay covering
  // it), so the drawer would only duplicate them there. Everywhere else (other
  // tabs, or any tab with a detail page open) the drawer is the surface.
  const showDecisionDrawer =
    decisions.length > 0 && (tab !== "decisions" || overlayOpen);

  if (!secret) {
    return (
      <div className={styles.gate}>
        <div className={styles.gateLogo}>F</div>
        <h1>{t("Fleet 移动端")}</h1>
        <p>
          {idbProbed
            ? t("请在桌面端 Fleet 的「移动端」板块扫码打开本页面（链接里带配对密钥）。")
            : t("正在恢复配对…")}
        </p>
      </div>
    );
  }

  // 配对失败的整屏拦截只在**只有一台**设备时成立:多台在册时,一台密钥失效不该
  // 把其他几台的卡一并挡在外面 —— 那一台的错误由「更多」页的连接状态如实呈现,
  // 用户可以在设备列表里把它移除或重新扫码。
  if (authError && runtimeDevices.length <= 1) {
    return (
      <div className={styles.gate}>
        <div className={styles.gateLogo}>F</div>
        <h1>{t("配对失败")}</h1>
        <p>{t("{0}。密钥可能已被重置，请回到桌面端重新扫码。", authError)}</p>
        <button
          className={styles.gateButton}
          onClick={() => {
            clearBook();
            clearCachedSessions(deviceId);
            location.reload();
          }}
        >
          {t("清除本机密钥")}
        </button>
      </div>
    );
  }

  return (
    // 整棵树跑在「当前作用域设备」里：里面所有按设备分家的本地存储（草稿、附件、
    // workspace 记忆）都从这里取命名空间，无需逐个 prop 往下传。
    <DeviceScopeProvider deviceId={deviceId}>
    {/* 每台设备一条连接。不渲染任何 DOM：它只是把那条 socket 的生命周期挂在
        React 树上，设备被移除时 key 消失、清理函数自然把它关掉。 */}
    {runtimeDevices.map((d, i) => (
      <DeviceConnection
        key={d.id}
        device={d}
        storageId={MOCK ? undefined : NEEDS_PAIRING ? d.id : null}
        makeTransport={makeTransport}
        dispatch={dispatch}
        registerHandle={registerHandle}
        hasPendingDecisions={(states[d.id]?.decisions.length ?? 0) > 0}
        agentOnline={states[d.id]?.agentOnline ?? false}
        isActive={d.id === activeDeviceId}
        index={i}
        visibility={visibility}
        connected={states[d.id]?.connected ?? false}
        pushGranted={push === "granted"}
        pushMuted={pushMuted[d.id] ?? true}
      />
    ))}
    <div className={styles.app}>
      <header className={styles.header}>
        <span className={styles.title}>Fleet</span>
        {todayUsage && (
          <span
            className={styles.usage}
            title={`${t("今日累计")} $${todayUsage.costUsd.toFixed(2)} · ${fmtTokens(todayUsage.inputTokens + todayUsage.outputTokens)} tok`}
          >
            <span className={styles.usageCost}>${todayUsage.costUsd.toFixed(2)}</span>
            <span className={styles.usageTokens}>
              {fmtTokens(todayUsage.inputTokens + todayUsage.outputTokens)}
            </span>
          </span>
        )}
        <span
          className={styles.connDot}
          data-state={!connected ? "offline" : agentOnline ? "online" : "agent-offline"}
          data-congestion={congestion}
        />
        <span
          className={styles.connLabel}
          title={rttSplit ? formatRttSplit(rttSplit, t) : undefined}
        >
          {!connected
            ? t("连接中…")
            : !agentOnline
              ? t("桌面端离线")
              : congestion === "congested"
                ? t("在线 · 网络拥挤")
                : congestion === "fair"
                  ? t("在线 · 网络一般")
                  : t("桌面端在线")}
        </span>
      </header>

      {/* 这条横幅讲的是「iOS 7 天不用会抹掉本地配对，得重新扫码」——同源形态
          根本没有配对可丢（后端就是发出这张页面的那个进程），显示它纯属误导。
          用 NEEDS_PAIRING 而不是 SUPPORTS_PUSH：这条说的是配对，不是推送。 */}
      {NEEDS_PAIRING && !MOCK && needsA2hsForDurableStorage() && !a2hsDismissed && (
        <div className={styles.pushBanner}>
          <span>
            {t(
              "用 Safari 分享菜单「添加到主屏幕」后从主屏幕打开——否则 7 天不访问，iOS 会清掉本机配对，需重新扫码。",
            )}
          </span>
          <button
            className={styles.pushButton}
            onClick={() => {
              localStorage.setItem(A2HS_DISMISSED_KEY, "1");
              setA2hsDismissed(true);
            }}
          >
            {t("知道了")}
          </button>
        </div>
      )}

      {!MOCK && push !== "granted" && push !== "unsupported" && push !== "unsupported-harmony" && (
        <div className={styles.pushBanner}>
          {push === "ios-needs-a2hs" ? (
            <span>{t("要接收通知，请先用 Safari 分享菜单「添加到主屏幕」，再从主屏幕打开。")}</span>
          ) : push === "denied" ? (
            <span>{t("通知权限已被拒绝，请在系统设置中为本站点重新开启。")}</span>
          ) : (
            <>
              <span>{t("开启通知，第一时间收到新决策卡。")}</span>
              <button className={styles.pushButton} onClick={handleEnablePush}>
                {t("开启")}
              </button>
            </>
          )}
        </div>
      )}

      <main className={styles.main}>
        {tab === "decisions" ? (
          <DecisionsView
            decisions={decisions}
            transportFor={transportFor}
            connected={connected}
            agentOnline={agentOnline}
            decisionsLoaded={decisionsLoaded}
            workspaceOf={workspaceOf}
            onAnswered={markAnswered}
            onOpenSession={openSessionRoot}
            deviceLabelOf={deviceLabelOf}
            focusDecision={focusDecision}
          />
        ) : tab === "tasks" ? (
          <TasksView
            sessions={mergedSessions}
            deviceLabelOf={runtimeDevices.length > 1 ? deviceLabelOf : undefined}
            client={client}
            connected={connected}
            agentOnline={agentOnline}
            sessionsLoaded={sessionsLoaded}
            onOpenSession={(s: WithDevice<SessionInfo>) => openSessionRoot(s.deviceId, s.id)}
            onMarkRead={markRead}
          />
        ) : tab === "wiki" ? (
          <WikiView
            client={client}
            onOpenDoc={(doc) => setWikiStack([{ deviceId: activeDeviceId, doc }])}
          />
        ) : (
          <MoreView
            endpointLabel={client?.endpointLabel ?? ""}
            devices={book.devices}
            activeDeviceId={deviceId}
            onSwitchDevice={switchDevice}
            onRenameDevice={renameDeviceLabel}
            onRemoveDevice={(d) => void removeDeviceEntry(d)}
            deviceMuted={(id) => pushMuted[id] ?? true}
            onMuteDevice={(d, muted) => void handleMuteDevice(d, muted)}
            onUnpairAll={unpairAll}
            supportsPush={SUPPORTS_PUSH}
            connected={connected}
            agentOnline={agentOnline}
            sessionsFrame={sessionsFrame}
            rttSplit={rttSplit}
            snapshotSources={snapshotSources}
            push={push}
            pushOptedOut={pushOptedOut}
            onEnablePush={handleEnablePush}
            onDisablePush={handleDisablePush}
            onOpenRepo={() => setShowRepo(true)}
            onOpenPlans={() => setShowPlans(true)}
            onOpenArtifacts={() => setShowArtifacts(true)}
            onOpenUsage={() => setShowUsage(true)}
          />
        )}
      </main>

      {/* 后退栈的底层：只要不在主页 tab，返回一次先回主页（安卓惯例，反复切 tab
          也只占一条历史），再返回才轮到栈底的退出确认。挂在浮层之前，保证浮层始终压在它上面。 */}
      {tab !== "decisions" && <HistoryLayer onBack={() => setTab("decisions")} />}

      {/* 每一层下钻占一层历史，但只渲染栈顶那层详情——底下几层不必挂着重复拉 tail。 */}
      {detailStack.map((_, i) => (
        <HistoryLayer key={i} onBack={() => setDetailStack((s) => s.slice(0, i))} />
      ))}
      {detailSession && (
        <SessionDetailView
          session={detailSession}
          // 详情页解析子代理/父会话都按 id 找,所以只给它**这条会话所属那一台**的
          // 列表 —— 混进别台的会话只会让它按同名 id 找到一条自己拉不动的记录。
          sessions={mergedSessions.filter((s) => s.deviceId === detailSession.deviceId)}
          client={transportFor(detailSession.deviceId)}
          onBack={() => setDetailStack((s) => s.slice(0, -1))}
          onOpenSessionId={(id: string) => openSessionById(detailSession.deviceId, id)}
          onDwellRead={() => markRead([detailSession])}
        />
      )}

      {/* 每篇文档占一层，但只渲染栈顶那篇——底下几篇不必挂着重复拉正文/渲 mermaid。 */}
      {wikiStack.map((_, i) => (
        <HistoryLayer key={i} onBack={() => setWikiStack((s) => s.slice(0, i))} />
      ))}
      {wikiStack.length > 0 && (
        <WikiDocView
          doc={wikiStack[wikiStack.length - 1].doc}
          client={transportFor(wikiStack[wikiStack.length - 1].deviceId)}
          onBack={() => setWikiStack((s) => s.slice(0, -1))}
          onOpenDoc={(doc) =>
            setWikiStack((s) => [...s, { deviceId: s[s.length - 1].deviceId, doc }])
          }
        />
      )}

      {showRepo && (
        <>
          <HistoryLayer onBack={() => setShowRepo(false)} />
          <RepoView
            client={client}
            onBack={() => setShowRepo(false)}
            onOpenRepo={(repo) => setRepoDetail({ deviceId: activeDeviceId, repo })}
          />
        </>
      )}

      {repoDetail && (
        <>
          <HistoryLayer onBack={() => setRepoDetail(null)} />
          <RepoDetailView
            repo={repoDetail.repo}
            client={transportFor(repoDetail.deviceId)}
            onBack={() => setRepoDetail(null)}
          />
        </>
      )}

      {showPlans && (
        <>
          <HistoryLayer onBack={() => setShowPlans(false)} />
          <PlansView
            sessions={scopedSessions}
            client={client}
            onBack={() => setShowPlans(false)}
          />
        </>
      )}

      {showArtifacts && (
        <>
          <HistoryLayer onBack={() => setShowArtifacts(false)} />
          <ArtifactsView
            client={client}
            onBack={() => setShowArtifacts(false)}
          />
        </>
      )}

      {showUsage && (
        <>
          <HistoryLayer onBack={() => setShowUsage(false)} />
          <UsageView
            client={client}
            todayUsage={todayUsage}
            onBack={() => setShowUsage(false)}
          />
        </>
      )}

      {showNewSession && (
        <>
          <HistoryLayer onBack={() => setShowNewSession(false)} />
          <NewSessionSheet
            // 新会话开在**当前作用域**那一台上,所以最近 workspace 也只取它的。
            sessions={scopedSessions}
            client={client}
            initialFiles={sharedFiles}
            relayReady={connected}
            onClose={() => {
              setShowNewSession(false);
              // Consumed by the sheet — don't re-upload them if it reopens.
              setSharedFiles([]);
            }}
          />
        </>
      )}

      {showDecisionDrawer && (
        <DecisionDrawer
          decisions={decisions}
          transportFor={transportFor}
          connected={connected}
          agentOnline={agentOnline}
          decisionsLoaded={decisionsLoaded}
          workspaceOf={workspaceOf}
          onAnswered={markAnswered}
          onOpenSession={openSessionRoot}
          deviceLabelOf={deviceLabelOf}
        />
      )}

      {exitArmed && <div className={styles.exitToast}>{t("再按一次返回退出")}</div>}

      <nav className={styles.tabs}>
        <button
          className={styles.tabButton}
          data-active={tab === "decisions"}
          onClick={() => setTab("decisions")}
        >
          <span className={styles.tabIcon}>
            <Inbox size={20} />
          </span>
          {t("决策")}
          {decisions.length > 0 && <span className={styles.badge}>{decisions.length}</span>}
        </button>
        <button
          className={styles.tabButton}
          data-active={tab === "tasks"}
          onClick={() => setTab("tasks")}
        >
          <span className={styles.tabIcon}>
            <ListChecks size={20} />
          </span>
          {t("任务")}
        </button>
        <button
          className={styles.centerTab}
          aria-label={t("新会话")}
          onClick={() => setShowNewSession(true)}
        >
          <span className={styles.centerFab}>
            <Plus size={24} />
          </span>
          <span className={styles.centerLabel}>{t("新会话")}</span>
        </button>
        <button
          className={styles.tabButton}
          data-active={tab === "wiki"}
          onClick={() => setTab("wiki")}
        >
          <span className={styles.tabIcon}>
            <BookOpen size={20} />
          </span>
          {t("知识库")}
        </button>
        <button
          className={styles.tabButton}
          data-active={tab === "more"}
          onClick={() => setTab("more")}
        >
          <span className={styles.tabIcon}>
            <MoreHorizontal size={20} />
          </span>
          {t("更多")}
        </button>
      </nav>
    </div>
    </DeviceScopeProvider>
  );
}
