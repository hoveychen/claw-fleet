import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { BookOpen, Inbox, ListChecks, MoreHorizontal, Plus } from "lucide-react";
import styles from "./App.module.css";
import {
  disablePush,
  enablePush,
  isPushOptedOut,
  pushState,
  unsubscribeChannel,
  type PushState,
} from "./push";
// 这里不再认识任何一个具体传输层。造哪一个由 main.tsx 按构建模式决定并注入
// —— 那处选择是一对动态 import，同源构建把 relay 那条分支连同整棵依赖树一起
// 消掉，而如果 App 静态 import 了 RelayClient，那套安排就白做了。
import type { FleetTransport, RttSample, TransportHandlers } from "./transport";
import { NEEDS_PAIRING, SUPPORTS_PUSH } from "./hostMode";
import {
  computeCongestion,
  formatRttSplit,
  splitRtt,
  RECONNECT_WINDOW_MS,
  type Congestion,
  type RttSplit,
} from "./connQuality";
import { agentKeyOf, reconcileDecisions } from "./decisionReconcile";
import { recordSnapshotSource, type SnapshotSource } from "./snapshotSources";
import { reconcilePlan } from "./reconcilePlan";
// 只取零依赖的开关。`?mock` 那个假客户端 extends RelayClient，从这里 import
// 会把整棵 relay 依赖树静态拖进同源构建 —— 造它的活儿归 transportRelay.ts。
import { isMockMode } from "./mockMode";
import type {
  DecisionKind,
  DecisionRequest,
  PendingDecision,
  PendingSnapshot,
  RepoSummary,
  SessionInfo,
  TodayUsage,
  WikiDoc,
} from "./types";
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
import {
  clearCachedSessions,
  loadCachedSessions,
  saveCachedSessions,
} from "./sessionCache";
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
    return adoptScannedDevice(stored, scanned.secret, newDeviceMint(stored), scanned.relayBase)
      .book;
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
  // ——那些内容只对某一台机器有意义（见 deviceScope.tsx）。
  const deviceId = current?.id ?? null;
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
      // relayBase 必须一起传下去 —— 壳的页面 origin 是 `capacitor://localhost`，
      // 丢了它就只能连打包时烧进去的那个 relay，自建 relay 永远配不上。
      setBook(
        (prev) =>
          adoptScannedDevice(prev, paired.secret, newDeviceMint(prev), paired.relayBase).book,
      );
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
      const live = clientRef.current;
      if (device.id === deviceIdRef.current && live?.isAuthed) {
        return unsubscribeChannel(live);
      }
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
  /// 通知点击要聚焦的决策卡。nonce 让「同一张卡被连点两次」也能触发。
  const [focusDecision, setFocusDecision] = useState<{ id: string; nonce: number } | null>(null);
  const [connected, setConnected] = useState(false);
  const [agentOnline, setAgentOnline] = useState(false);
  // Whether the desktop's sessions delta path is engaged, surfaced in More.
  // `last` is the most recent frame kind; the counts accumulate for the session.
  const [sessionsFrame, setSessionsFrame] = useState<{
    last: "full" | "delta" | null;
    full: number;
    delta: number;
  }>({ last: null, full: 0, delta: 0 });
  // Header congestion light. Raw signals live in refs (RTT of the last reply,
  // timestamps of recent reconnects) so they don't each force a re-render; the
  // level is recomputed and set explicitly when a sample or reconnect lands.
  const [congestion, setCongestion] = useState<Congestion>("good");
  // The latest round trip attributed to phone link / desktop link / desktop
  // handler. State (not a ref) because the More page renders it: the total says
  // "it's slow", only the split says which of the three to go fix.
  const [rttSplit, setRttSplit] = useState<RttSplit | null>(null);
  const rttRef = useRef<number | null>(null);
  const reconnectsRef = useRef<number[]>([]);
  const [authError, setAuthError] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  // Flips true the first time a `sessions` snapshot arrives, so the task page
  // can tell "still waiting for the first push" from "pushed, genuinely empty".
  const [sessionsLoaded, setSessionsLoaded] = useState(false);

  // Cold-start hydrate: paint the last cached task list immediately so the page
  // isn't blank while the socket (re)connects and delivers the first full
  // snapshot. Seed only if the live snapshot hasn't already landed — the
  // functional update keeps fresh data if `onSessions` won the race. `sessions`
  // being non-empty while `sessionsLoaded` is still false is exactly the
  // "showing cache, awaiting first live push" state the task page labels.
  useEffect(() => {
    if (MOCK) return;
    // 切设备先清场：上一台的任务列表、决策卡、今日用量、可信 agent 判定都只对
    // 上一台成立。留着它们不是「先显示旧数据再刷新」——那些卡上的 id 在新设备
    // 上根本不存在，点一下就是一次打错地方的请求。首次挂载时这些本来就是空的。
    setSessions([]);
    setSessionsLoaded(false);
    setDecisions([]);
    setDecisionsLoaded(false);
    setTodayUsage(null);
    trustedAgentRef.current = undefined;
    answeredRef.current = new Map();
    snapshotSourcesRef.current = [];
    let cancelled = false;
    void loadCachedSessions(deviceId).then((cached) => {
      if (cancelled || !cached?.length) return;
      setSessions((prev) => (prev.length === 0 ? cached : prev));
    });
    return () => {
      cancelled = true;
    };
  }, [deviceId]);
  const [todayUsage, setTodayUsage] = useState<TodayUsage | null>(null);
  const [decisions, setDecisions] = useState<PendingDecision[]>([]);
  // Mirrors `sessionsLoaded`: flips true once the first authoritative pending
  // snapshot (or a live decision event) has landed, so the decisions tab can
  // tell "still awaiting the first push" (show skeleton cards) from "pushed,
  // genuinely nothing pending" (show the empty state).
  const [decisionsLoaded, setDecisionsLoaded] = useState(false);
  const [push, setPush] = useState<PushState>(pushState);
  // Sub-state below "granted": the user can turn notifications off even while
  // the browser permission stays granted. Persisted so it survives reloads.
  const [pushOptedOut, setPushOptedOut] = useState<boolean>(isPushOptedOut);
  // 会话详情是一条下钻链而不是单页：从 Agent 卡「打开子代理」往上叠一层子代理会话，
  // 返回逐层退回（和知识库 wikiStack 同一套栈式浮层模型）。栈顶 id 解析出当前详情。
  const [detailStack, setDetailStack] = useState<string[]>([]);
  // 知识库文档是一条链而不是单页：`[[slug]]` 站内跳转往上叠一篇，返回逐篇退回。
  const [wikiStack, setWikiStack] = useState<WikiDoc[]>([]);
  const [showRepo, setShowRepo] = useState(false);
  const [repoDetail, setRepoDetail] = useState<RepoSummary | null>(null);
  const [showUsage, setShowUsage] = useState(false);
  const [showPlans, setShowPlans] = useState(false);
  const [showArtifacts, setShowArtifacts] = useState(false);
  const [showNewSession, setShowNewSession] = useState(false);
  // Files handed over by another app's share, pending upload once the
  // new-session sheet mounts (that's where the attachment state lives).
  const [sharedFiles, setSharedFiles] = useState<File[]>([]);
  const [exitArmed, setExitArmed] = useState(false);
  const clientRef = useRef<FleetTransport | null>(null);
  // ids answered on THIS device whose answer is still in flight → timestamp.
  // Suppresses the just-answered card from flickering back when a fallback
  // `pending_snapshot` (which lags the send on a slow link) still lists it.
  const answeredRef = useRef<Map<string, number>>(new Map());
  // Fingerprint of the agent whose snapshots actually carry this machine's
  // cards. The relay fans every request out to all agents in the channel, so
  // "which agent answered" is the only way to tell the desktop's authoritative
  // empty list from a stray agent's (decisionReconcile.ts).
  const trustedAgentRef = useRef<string | undefined>(undefined);
  // Every agent that has answered a snapshot, for the More tab's diagnostics.
  const snapshotSourcesRef = useRef<SnapshotSource[]>([]);

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

  const addDecision = useCallback((kind: DecisionKind, request: unknown) => {
    const req = request as DecisionRequest;
    if (!req?.id) return;
    // A live decision frame is proof the pipe is delivering — clear the skeleton
    // even if the initial snapshot request hasn't returned yet.
    setDecisionsLoaded(true);
    setDecisions((prev) => {
      if (prev.some((d) => d.id === req.id)) return prev;
      return [...prev, { kind, id: req.id, request: req, arrivedAt: Date.now() }];
    });
  }, []);

  // Local answer sent from this device: drop the card now (optimistic) and
  // remember the id so a lagging snapshot can't resurrect it before the desktop
  // confirms. ANSWER_GRACE_MS is the fallback ceiling for a lost answer frame.
  const markAnswered = useCallback((id: string) => {
    answeredRef.current.set(id, Date.now());
    setDecisions((prev) => prev.filter((d) => d.id !== id));
  }, []);

  const removeDecision = useCallback((_kind: DecisionKind, id: string) => {
    // Authoritative resolution (desktop/another device) — the in-flight-answer
    // bookkeeping for this id is moot, clear it so it can't linger.
    answeredRef.current.delete(id);
    setDecisions((prev) => prev.filter((d) => d.id !== id));
  }, []);

  const refreshPending = useCallback(async (client: FleetTransport) => {
    try {
      const snap = await client.request<PendingSnapshot>("pending_snapshot");
      const kinds: Array<[DecisionKind, DecisionRequest[] | undefined]> = [
        ["guard", snap.guard],
        ["elicitation", snap.elicitation],
        ["fleet-ask", snap.fleetAsk],
        ["plan-approval", snap.planApproval],
        ["permission-prompt", snap.permissionPrompt],
        ["a2ui-render", snap.a2uiRender],
      ];
      const fresh: PendingDecision[] = [];
      for (const [kind, list] of kinds) {
        for (const request of list ?? []) {
          fresh.push({ kind, id: request.id, request, arrivedAt: Date.now() });
        }
      }
      // Snapshot is authoritative, but a card answered on this device whose
      // answer is still in flight must not pop back in (reconcileDecisions
      // suppresses it until confirmed gone, or the grace window lapses) — and
      // an "empty" from an agent other than our desktop is not authoritative at
      // all (the relay fans requests out to every agent in the channel; see
      // decisionReconcile.ts).
      const agentKey = agentKeyOf(snap.agent);
      setDecisions((prev) => {
        const { decisions, answeredAt, ignored, trustedAgentKey } = reconcileDecisions({
          prev,
          fresh,
          answeredAt: answeredRef.current,
          now: Date.now(),
          agentKey,
          trustedAgentKey: trustedAgentRef.current,
        });
        answeredRef.current = answeredAt;
        trustedAgentRef.current = trustedAgentKey;
        // Diagnostics log, kept in a ref alongside the two above (a nested
        // setState here would run inside an updater). The More tab renders it,
        // so a second agent that blanked — or tried to blank — the card list
        // leaves a trace Boss can read off the phone.
        snapshotSourcesRef.current = recordSnapshotSource(snapshotSourcesRef.current, {
          key: agentKey,
          agent: snap.agent,
          at: Date.now(),
          trusted: !!agentKey && agentKey === trustedAgentKey,
          ignored: !!ignored,
        });
        return decisions;
      });
      // First authoritative snapshot is in — retire the loading skeleton.
      setDecisionsLoaded(true);
    } catch {
      // agent offline — live events will catch us up later
    }
  }, []);

  // Recompute the header congestion level from the latest RTT sample and the
  // reconnects still inside the recent window. Called whenever either signal
  // moves; the 20s today_usage poll keeps RTT samples flowing, so the level
  // decays back down once a weak link recovers.
  const recomputeCongestion = useCallback(() => {
    const now = Date.now();
    reconnectsRef.current = reconnectsRef.current.filter(
      (ts) => now - ts < RECONNECT_WINDOW_MS,
    );
    setCongestion(computeCongestion(rttRef.current, reconnectsRef.current.length));
  }, []);

  useEffect(() => {
    if (!secret) return;
    const handlers = {
      onStatus: setConnected,
      onAgentOnline: (online: boolean) => {
        setAgentOnline(online);
        if (online && clientRef.current) {
          void refreshPending(clientRef.current);
        }
      },
      onDecisionCreated: addDecision,
      onDecisionResolved: removeDecision,
      onSessions: (list: SessionInfo[]) => {
        setSessions(list);
        setSessionsLoaded(true);
        // Persist the merged full list so the next cold start paints it. Both
        // full and delta frames arrive here already merged (see relay.ts), so
        // the cache always holds the latest complete snapshot.
        saveCachedSessions(deviceId, list);
      },
      onSessionsKind: (kind: "full" | "delta") =>
        setSessionsFrame((s) => ({
          last: kind,
          full: s.full + (kind === "full" ? 1 : 0),
          delta: s.delta + (kind === "delta" ? 1 : 0),
        })),
      onRttSample: (sample: RttSample) => {
        // The congestion light still judges the whole round trip — what the user
        // feels is the total, whichever segment ate it. The segments are for
        // telling them *which* one did, on the More page.
        rttRef.current = sample.totalMs;
        setRttSplit(splitRtt(sample));
        recomputeCongestion();
      },
      onReconnect: () => {
        reconnectsRef.current.push(Date.now());
        recomputeCongestion();
      },
      onAuthError: (message: string) => setAuthError(message),
    };
    const client = makeTransport(secret, handlers, relayBase);
    clientRef.current = client;
    client.connect();
    // Mobile browsers may never run React cleanup on tab close — tell the
    // desktop we're leaving on pagehide too (desktop timeout is the backstop).
    const onPageHide = () => client.sayGoodbye();
    window.addEventListener("pagehide", onPageHide);
    return () => {
      window.removeEventListener("pagehide", onPageHide);
      client.close();
    };
    // deviceId / relayBase 与 secret 总是同时变（它们都来自当前那台设备），列进
    // 依赖是为了让「切设备」这件事在这里显式成立，而不是靠 secret 恰好也变了。
  }, [
    secret,
    deviceId,
    relayBase,
    makeTransport,
    addDecision,
    removeDecision,
    refreshPending,
    recomputeCongestion,
  ]);

  // Today's cumulative token/cost counter in the header. Polls while the desktop
  // is online; the desktop computes the figure (agent sessions created today +
  // Fleet's own LLM spend) so mobile just displays the single number.
  useEffect(() => {
    if (!agentOnline) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      const client = clientRef.current;
      if (client?.isAuthed) {
        try {
          const u = await client.request<TodayUsage>("today_usage");
          if (!cancelled) setTodayUsage(u);
        } catch {
          /* transient — keep the last value */
        }
      }
      if (!cancelled) timer = window.setTimeout(poll, 20_000);
    };
    void poll();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentOnline]);

  // Re-sync when the PWA returns to the foreground (mobile browsers freeze
  // sockets aggressively; the reconnect fires but the snapshot may be stale).
  // Also recompute the push state: Notification.permission can change while we
  // were backgrounded (Boss toggled it in system/browser settings), and the
  // banner reads pushState() only at mount — without this it stays stale, e.g.
  // still showing "已拒绝" after the site was re-allowed.
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      setPush(pushState());
      if (clientRef.current?.isAuthed) void refreshPending(clientRef.current);
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
  }, [refreshPending]);

  // Foreground fallback reconcile for pending decisions. Both `decision_created`
  // and `decision_resolved` are best-effort agent→client WS broadcasts with no
  // ack and no retry; the relay drops them if the phone's socket is down (a weak
  // link / network switch — the only way to miss a frame over TCP). The one-shot
  // `refreshPending` on (re)connect / foreground is meant to catch up, but on a
  // weak link that single pull can time out and is swallowed, with no retry — so
  // a missed card would strand forever. This loop is the durable backstop: it
  // keeps re-pulling the authoritative snapshot while the desktop is online and
  // the tab is visible, fast (3s) while cards are open so an answered-elsewhere
  // card clears promptly, slow (15s) when idle so a card missed during a drop is
  // recovered on the next tick. Interval and enablement come from reconcilePlan.
  useEffect(() => {
    const plan = reconcilePlan(agentOnline, decisions.length > 0);
    if (!plan.poll) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      const client = clientRef.current;
      if (client?.isAuthed && document.visibilityState === "visible") {
        await refreshPending(client);
      }
      if (!cancelled) timer = window.setTimeout(tick, plan.intervalMs);
    };
    timer = window.setTimeout(tick, plan.intervalMs);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentOnline, decisions.length, refreshPending]);

  // When permission lands on "granted" (fresh grant, or re-allowed in settings
  // after a denial), make sure a live subscription actually exists on the
  // relay — otherwise the banner just vanishes while no push is wired.
  // enablePush is idempotent: it reuses an existing subscription and only
  // creates one when missing, and requestPermission() no-ops once granted.
  // Skip the auto-(re)subscribe when the user has explicitly opted out —
  // otherwise a mount/focus would immediately resurrect the subscription they
  // just turned off (browser permission stays "granted" after disabling).
  //
  // `connected` is a dependency, not a convenience: `pushSubscribe` writes
  // straight to the socket and returns false when it isn't OPEN, with no queue
  // and no retry. This effect used to run only on `[push, pushOptedOut]`, and
  // the reconnect path was a lone `resyncPush(client)` fired immediately after
  // `client.connect()` — i.e. before the socket could possibly be open. So the
  // very first registration raced the handshake, and every later reconnect
  // (relay redeploy, network flap, waking from the background freeze) re-sent
  // nothing at all. Observed on-device: the phone held a valid push token,
  // reported itself subscribed, and the relay's subscription store never
  // gained the entry.
  useEffect(() => {
    if (connected && push === "granted" && !pushOptedOut && clientRef.current) {
      void enablePush(clientRef.current, relayBase);
    }
  }, [connected, push, pushOptedOut]);

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
    });
  }, []);

  const handleEnablePush = useCallback(async () => {
    if (!clientRef.current) return;
    setPush(await enablePush(clientRef.current, relayBase));
    setPushOptedOut(false);
  }, []);

  const handleDisablePush = useCallback(async () => {
    if (!clientRef.current) return;
    await disablePush(clientRef.current);
    setPushOptedOut(true);
  }, []);

  // Optimistic read stamps: the server re-derives lastReadMs on its next scan,
  // so until that push arrives we overlay the local click/dwell time.
  const [localReadMs, setLocalReadMs] = useState<Record<string, number>>({});
  const markRead = useCallback((items: SessionInfo[]) => {
    if (items.length === 0) return;
    clientRef.current
      ?.request("session_read", {
        items: items.map((s) => ({ sessionId: s.id, workspacePath: s.workspacePath })),
      })
      .catch(() => {});
    const now = Date.now();
    setLocalReadMs((prev) => {
      const next = { ...prev };
      for (const s of items) next[s.id] = now;
      return next;
    });
  }, []);

  const mergedSessions = useMemo(
    () =>
      sessions.map((s) => {
        const local = localReadMs[s.id];
        return local && local > (s.lastReadMs ?? 0) ? { ...s, lastReadMs: local } : s;
      }),
    [sessions, localReadMs],
  );

  const workspaceOf = useMemo(() => {
    const map = new Map<string, SessionInfo>();
    for (const s of mergedSessions) map.set(s.id, s);
    return (sessionId: string) => map.get(sessionId);
  }, [mergedSessions]);

  // The detail page re-derives its session from the live snapshot so status /
  // plan chips stay fresh while it is open. Top of the drill-down stack wins.
  const detailSession = useMemo(() => {
    const topId = detailStack[detailStack.length - 1];
    return topId ? (mergedSessions.find((s) => s.id === topId) ?? null) : null;
  }, [mergedSessions, detailStack]);

  // Open a session as a fresh root detail (from the tasks / decisions lists).
  const openSessionRoot = useCallback((id: string) => setDetailStack([id]), []);
  // Drill into a subagent from within a detail view — push another layer so the
  // back button returns to the opener. `AgentNav.open` prefixes `agent-`.
  const openSessionById = useCallback(
    (id: string) => setDetailStack((s) => [...s, id]),
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

  if (authError) {
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
            client={clientRef.current}
            connected={connected}
            agentOnline={agentOnline}
            decisionsLoaded={decisionsLoaded}
            workspaceOf={workspaceOf}
            onAnswered={markAnswered}
            onOpenSession={openSessionRoot}
            focusDecision={focusDecision}
          />
        ) : tab === "tasks" ? (
          <TasksView
            sessions={mergedSessions}
            client={clientRef.current}
            connected={connected}
            agentOnline={agentOnline}
            sessionsLoaded={sessionsLoaded}
            onOpenSession={(s) => openSessionRoot(s.id)}
            onMarkRead={markRead}
          />
        ) : tab === "wiki" ? (
          <WikiView client={clientRef.current} onOpenDoc={(doc) => setWikiStack([doc])} />
        ) : (
          <MoreView
            endpointLabel={clientRef.current?.endpointLabel ?? ""}
            devices={book.devices}
            activeDeviceId={deviceId}
            onSwitchDevice={switchDevice}
            onRenameDevice={renameDeviceLabel}
            onRemoveDevice={(d) => void removeDeviceEntry(d)}
            onUnpairAll={unpairAll}
            supportsPush={SUPPORTS_PUSH}
            connected={connected}
            agentOnline={agentOnline}
            sessionsFrame={sessionsFrame}
            rttSplit={rttSplit}
            snapshotSources={snapshotSourcesRef.current}
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
          sessions={mergedSessions}
          client={clientRef.current}
          onBack={() => setDetailStack((s) => s.slice(0, -1))}
          onOpenSessionId={openSessionById}
          onDwellRead={() => markRead([detailSession])}
        />
      )}

      {/* 每篇文档占一层，但只渲染栈顶那篇——底下几篇不必挂着重复拉正文/渲 mermaid。 */}
      {wikiStack.map((_, i) => (
        <HistoryLayer key={i} onBack={() => setWikiStack((s) => s.slice(0, i))} />
      ))}
      {wikiStack.length > 0 && (
        <WikiDocView
          doc={wikiStack[wikiStack.length - 1]}
          client={clientRef.current}
          onBack={() => setWikiStack((s) => s.slice(0, -1))}
          onOpenDoc={(doc) => setWikiStack((s) => [...s, doc])}
        />
      )}

      {showRepo && (
        <>
          <HistoryLayer onBack={() => setShowRepo(false)} />
          <RepoView
            client={clientRef.current}
            onBack={() => setShowRepo(false)}
            onOpenRepo={setRepoDetail}
          />
        </>
      )}

      {repoDetail && (
        <>
          <HistoryLayer onBack={() => setRepoDetail(null)} />
          <RepoDetailView
            repo={repoDetail}
            client={clientRef.current}
            onBack={() => setRepoDetail(null)}
          />
        </>
      )}

      {showPlans && (
        <>
          <HistoryLayer onBack={() => setShowPlans(false)} />
          <PlansView
            sessions={mergedSessions}
            client={clientRef.current}
            onBack={() => setShowPlans(false)}
          />
        </>
      )}

      {showArtifacts && (
        <>
          <HistoryLayer onBack={() => setShowArtifacts(false)} />
          <ArtifactsView
            client={clientRef.current}
            onBack={() => setShowArtifacts(false)}
          />
        </>
      )}

      {showUsage && (
        <>
          <HistoryLayer onBack={() => setShowUsage(false)} />
          <UsageView
            client={clientRef.current}
            todayUsage={todayUsage}
            onBack={() => setShowUsage(false)}
          />
        </>
      )}

      {showNewSession && (
        <>
          <HistoryLayer onBack={() => setShowNewSession(false)} />
          <NewSessionSheet
            sessions={mergedSessions}
            client={clientRef.current}
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
          client={clientRef.current}
          connected={connected}
          agentOnline={agentOnline}
          decisionsLoaded={decisionsLoaded}
          workspaceOf={workspaceOf}
          onAnswered={markAnswered}
          onOpenSession={openSessionRoot}
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
