import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { BookOpen, Inbox, ListChecks, MoreHorizontal, Plus } from "lucide-react";
import styles from "./App.module.css";
import { enablePush, pushState, resyncPush, type PushState } from "./push";
import { deviceLabel } from "./deviceLabel";
import { getClientId } from "./clientId";
import { RelayClient } from "./relay";
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
import { useI18n } from "./i18n";
import { NewSessionSheet } from "./views/Composer";
import { DecisionsView } from "./views/DecisionsView";
import { MoreView } from "./views/MoreView";
import { RepoView } from "./views/RepoView";
import { RepoDetailView } from "./views/RepoDetailView";
import { SessionDetailView } from "./views/SessionDetailView";
import { TasksView } from "./views/TasksView";
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

export function App() {
  // 订阅语言切换：App 根重渲即可带动整树（无 React.memo），各处 t() 现算。
  const { t } = useI18n();
  const [secret, setSecret] = useState<string | null>(loadSecretSync);
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
  const [tab, setTab] = useState<Tab>("decisions");
  const [connected, setConnected] = useState(false);
  const [agentOnline, setAgentOnline] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [todayUsage, setTodayUsage] = useState<TodayUsage | null>(null);
  const [decisions, setDecisions] = useState<PendingDecision[]>([]);
  const [push, setPush] = useState<PushState>(pushState);
  const [detailSessionId, setDetailSessionId] = useState<string | null>(null);
  const [wikiDoc, setWikiDoc] = useState<WikiDoc | null>(null);
  const [showRepo, setShowRepo] = useState(false);
  const [repoDetail, setRepoDetail] = useState<RepoSummary | null>(null);
  const [showNewSession, setShowNewSession] = useState(false);
  const clientRef = useRef<RelayClient | null>(null);

  const addDecision = useCallback((kind: DecisionKind, request: unknown) => {
    const req = request as DecisionRequest;
    if (!req?.id) return;
    setDecisions((prev) => {
      if (prev.some((d) => d.id === req.id)) return prev;
      return [...prev, { kind, id: req.id, request: req, arrivedAt: Date.now() }];
    });
  }, []);

  const removeDecision = useCallback((_kind: DecisionKind, id: string) => {
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
      // Snapshot is authoritative: replaces the list (dismissals included).
      // Keep the original arrival stamp for ids we already had, so refreshes
      // don't reshuffle the card order.
      setDecisions((prev) => {
        const seen = new Map(prev.map((d) => [d.id, d.arrivedAt]));
        return fresh.map((d) => ({ ...d, arrivedAt: seen.get(d.id) ?? d.arrivedAt }));
      });
    } catch {
      // agent offline — live events will catch us up later
    }
  }, []);

  useEffect(() => {
    if (!secret) return;
    const client = new RelayClient(
      secret,
      {
        onStatus: setConnected,
        onAgentOnline: (online) => {
          setAgentOnline(online);
          if (online && clientRef.current) {
            void refreshPending(clientRef.current);
          }
        },
        onDecisionCreated: addDecision,
        onDecisionResolved: removeDecision,
        onSessions: setSessions,
        onAuthError: (message) => setAuthError(message),
      },
      // Announced on every heartbeat so `pushSubscribed` stays current — read
      // live rather than captured at construction.
      () => {
        const { label, platform } = deviceLabel(navigator.userAgent);
        return { clientId: getClientId(), label, platform, pushSubscribed: pushState() === "granted" };
      },
    );
    clientRef.current = client;
    client.connect();
    void resyncPush(client);
    // Mobile browsers may never run React cleanup on tab close — tell the
    // desktop we're leaving on pagehide too (desktop timeout is the backstop).
    const onPageHide = () => client.sayGoodbye();
    window.addEventListener("pagehide", onPageHide);
    return () => {
      window.removeEventListener("pagehide", onPageHide);
      client.close();
    };
  }, [secret, addDecision, removeDecision, refreshPending]);

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

  // When permission lands on "granted" (fresh grant, or re-allowed in settings
  // after a denial), make sure a live subscription actually exists on the
  // relay — otherwise the banner just vanishes while no push is wired.
  // enablePush is idempotent: it reuses an existing subscription and only
  // creates one when missing, and requestPermission() no-ops once granted.
  useEffect(() => {
    if (push === "granted" && clientRef.current) {
      void enablePush(clientRef.current);
    }
  }, [push]);

  const handleEnablePush = useCallback(async () => {
    if (!clientRef.current) return;
    setPush(await enablePush(clientRef.current));
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
  // plan chips stay fresh while it is open.
  const detailSession = useMemo(
    () => (detailSessionId ? mergedSessions.find((s) => s.id === detailSessionId) ?? null : null),
    [mergedSessions, detailSessionId],
  );

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
            title={`${t("今日累计")} $${todayUsage.costUsd.toFixed(2)} · ${fmtTokens(todayUsage.outputTokens)} tok`}
          >
            <span className={styles.usageCost}>${todayUsage.costUsd.toFixed(2)}</span>
            <span className={styles.usageTokens}>{fmtTokens(todayUsage.outputTokens)}</span>
          </span>
        )}
        <span
          className={styles.connDot}
          data-state={!connected ? "offline" : agentOnline ? "online" : "agent-offline"}
        />
        <span className={styles.connLabel}>
          {!connected ? t("连接中…") : agentOnline ? t("桌面端在线") : t("桌面端离线")}
        </span>
      </header>

      {needsA2hsForDurableStorage() && !a2hsDismissed && (
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

      {push !== "granted" && push !== "unsupported" && push !== "unsupported-harmony" && (
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
            agentOnline={agentOnline}
            workspaceOf={workspaceOf}
            onAnswered={(id) => setDecisions((prev) => prev.filter((d) => d.id !== id))}
            onOpenSession={setDetailSessionId}
          />
        ) : tab === "tasks" ? (
          <TasksView
            sessions={mergedSessions}
            client={clientRef.current}
            onOpenSession={(s) => setDetailSessionId(s.id)}
            onMarkRead={markRead}
            onNewSession={() => setShowNewSession(true)}
          />
        ) : tab === "wiki" ? (
          <WikiView client={clientRef.current} onOpenDoc={setWikiDoc} />
        ) : (
          <MoreView
            connected={connected}
            agentOnline={agentOnline}
            push={push}
            onEnablePush={handleEnablePush}
            onOpenRepo={() => setShowRepo(true)}
          />
        )}
      </main>

      {detailSession && (
        <SessionDetailView
          session={detailSession}
          client={clientRef.current}
          onBack={() => setDetailSessionId(null)}
          onDwellRead={() => markRead([detailSession])}
        />
      )}

      {wikiDoc && (
        <WikiDocView
          doc={wikiDoc}
          client={clientRef.current}
          onBack={() => setWikiDoc(null)}
          onOpenDoc={setWikiDoc}
        />
      )}

      {showRepo && (
        <RepoView
          client={clientRef.current}
          onBack={() => setShowRepo(false)}
          onOpenRepo={setRepoDetail}
        />
      )}

      {repoDetail && (
        <RepoDetailView
          repo={repoDetail}
          client={clientRef.current}
          onBack={() => setRepoDetail(null)}
        />
      )}

      {showNewSession && (
        <NewSessionSheet
          sessions={mergedSessions}
          client={clientRef.current}
          onClose={() => setShowNewSession(false)}
        />
      )}

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
