import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { BookOpen, Inbox, ListChecks, MoreHorizontal, Plus } from "lucide-react";
import styles from "./App.module.css";
import {
  disablePush,
  enablePush,
  isPushOptedOut,
  pushState,
  resyncPush,
  type PushState,
} from "./push";
import { deviceLabel } from "./deviceLabel";
import { getClientId } from "./clientId";
import { RelayClient, gzipSupported, binarySupported } from "./relay";
import { computeCongestion, RECONNECT_WINDOW_MS, type Congestion } from "./connQuality";
import { reconcileDecisions } from "./decisionReconcile";
import { reconcilePlan } from "./reconcilePlan";
import { MockRelayClient, isMockMode } from "./mock/relay";
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
import {
  clearSecret,
  loadSecretFromIdb,
  loadSecretSync,
  needsA2hsForDurableStorage,
  persistSecret,
} from "./secretStore";
import { onPairingLink } from "./deepLink";
import {
  clearCachedSessions,
  loadCachedSessions,
  saveCachedSessions,
} from "./sessionCache";
import { useI18n } from "./i18n";
import { ExitGuard, installUnloadPrompt } from "./exitGuard";
import { HistoryLayer, setRootBackHandler } from "./useNavStack";
import { NEW_SESSION_DRAFT_KEY, NewSessionSheet } from "./views/Composer";
import { loadDraft, saveDraft } from "./draft";
import { onShareReceived } from "./shareTarget";
import { DecisionsView } from "./views/DecisionsView";
import { DecisionDrawer } from "./views/DecisionDrawer";
import { MoreView } from "./views/MoreView";
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

// `?mock` runs the whole app on fixtures (no relay, no pairing) — used by the
// promo screen-recording pipeline and for quick UI work in a plain browser.
const MOCK = isMockMode();

export function App() {
  // 订阅语言切换：App 根重渲即可带动整树（无 React.memo），各处 t() 现算。
  const { t } = useI18n();
  // `?mock` stands in for a pairing secret so the gate below opens and the
  // effect that builds the client runs — it just builds a MockRelayClient.
  const [secret, setSecret] = useState<string | null>(() =>
    MOCK ? "mock-secret" : loadSecretSync(),
  );
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
    loadSecretFromIdb().then((s) => {
      if (cancelled) return;
      if (s) {
        persistSecret(s);
        setSecret(s);
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
      persistSecret(paired);
      setSecret(paired);
      setIdbProbed(true);
    });
  }, []);

  // Android share sheet → new-session composer. Seed the draft *before* opening
  // the sheet: NewSessionSheet reads it once on mount via useDraft, so writing
  // after would be ignored. Merging (rather than replacing) keeps whatever
  // workspace/model the user last picked.
  useEffect(() => {
    return onShareReceived((prompt) => {
      const current = loadDraft<Record<string, unknown>>(NEW_SESSION_DRAFT_KEY, {});
      saveDraft(NEW_SESSION_DRAFT_KEY, { ...current, prompt });
      setShowNewSession(true);
    });
  }, []);

  const [tab, setTab] = useState<Tab>("decisions");
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
    let cancelled = false;
    void loadCachedSessions().then((cached) => {
      if (cancelled || !cached?.length) return;
      setSessions((prev) => (prev.length === 0 ? cached : prev));
    });
    return () => {
      cancelled = true;
    };
  }, []);
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
  const [showNewSession, setShowNewSession] = useState(false);
  const [exitArmed, setExitArmed] = useState(false);
  const clientRef = useRef<RelayClient | null>(null);
  // ids answered on THIS device whose answer is still in flight → timestamp.
  // Suppresses the just-answered card from flickering back when a fallback
  // `pending_snapshot` (which lags the send on a slow link) still lists it.
  const answeredRef = useRef<Map<string, number>>(new Map());

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

  const refreshPending = useCallback(async (client: RelayClient) => {
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
      // suppresses it until confirmed gone, or the grace window lapses).
      setDecisions((prev) => {
        const { decisions, answeredAt } = reconcileDecisions({
          prev,
          fresh,
          answeredAt: answeredRef.current,
          now: Date.now(),
        });
        answeredRef.current = answeredAt;
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
        saveCachedSessions(list);
      },
      onSessionsKind: (kind: "full" | "delta") =>
        setSessionsFrame((s) => ({
          last: kind,
          full: s.full + (kind === "full" ? 1 : 0),
          delta: s.delta + (kind === "delta" ? 1 : 0),
        })),
      onRttSample: (rttMs: number) => {
        rttRef.current = rttMs;
        recomputeCongestion();
      },
      onReconnect: () => {
        reconnectsRef.current.push(Date.now());
        recomputeCongestion();
      },
      onAuthError: (message: string) => setAuthError(message),
    };
    const client = MOCK
      ? new MockRelayClient(handlers)
      : new RelayClient(
          secret,
          handlers,
          // Announced on every heartbeat so `pushSubscribed` stays current — read
          // live rather than captured at construction.
          () => {
            const { label, platform } = deviceLabel(navigator.userAgent);
            return {
              clientId: getClientId(),
              label,
              platform,
              pushSubscribed: pushState() === "granted",
              supportsGzip: gzipSupported(),
              supportsBinary: binarySupported(),
              // Delta application is pure JS (see relay.ts sessions_delta) — no
              // browser API to feature-detect, so always true.
              supportsDelta: true,
              // Static per build; lets the desktop flag a stale bundle.
              appCommit: __APP_COMMIT__,
            };
          },
        );
    clientRef.current = client;
    client.connect();
    if (!MOCK) void resyncPush(client);
    // Mobile browsers may never run React cleanup on tab close — tell the
    // desktop we're leaving on pagehide too (desktop timeout is the backstop).
    const onPageHide = () => client.sayGoodbye();
    window.addEventListener("pagehide", onPageHide);
    return () => {
      window.removeEventListener("pagehide", onPageHide);
      client.close();
    };
  }, [secret, addDecision, removeDecision, refreshPending, recomputeCongestion]);

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
  useEffect(() => {
    if (push === "granted" && !pushOptedOut && clientRef.current) {
      void enablePush(clientRef.current);
    }
  }, [push, pushOptedOut]);

  const handleEnablePush = useCallback(async () => {
    if (!clientRef.current) return;
    setPush(await enablePush(clientRef.current));
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
            clearSecret();
            clearCachedSessions();
            location.reload();
          }}
        >
          {t("清除本机密钥")}
        </button>
      </div>
    );
  }

  return (
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
        <span className={styles.connLabel}>
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

      {!MOCK && needsA2hsForDurableStorage() && !a2hsDismissed && (
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
            connected={connected}
            agentOnline={agentOnline}
            sessionsFrame={sessionsFrame}
            push={push}
            pushOptedOut={pushOptedOut}
            onEnablePush={handleEnablePush}
            onDisablePush={handleDisablePush}
            onOpenRepo={() => setShowRepo(true)}
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
            onClose={() => setShowNewSession(false)}
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
  );
}
