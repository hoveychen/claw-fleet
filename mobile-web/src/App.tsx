import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import styles from "./App.module.css";
import { enablePush, pushState, resyncPush, type PushState } from "./push";
import { RelayClient } from "./relay";
import type {
  DecisionKind,
  DecisionRequest,
  PendingDecision,
  PendingSnapshot,
  SessionInfo,
} from "./types";
import { DecisionsView } from "./views/DecisionsView";
import { SessionDetailView } from "./views/SessionDetailView";
import { TasksView } from "./views/TasksView";

const SECRET_KEY = "fleet-relay-secret";

/** Take the secret out of `#k=…`, persist it, and scrub it from the URL. */
function resolveSecret(): string | null {
  const match = window.location.hash.match(/[#&]k=([A-Za-z0-9_-]+)/);
  if (match) {
    localStorage.setItem(SECRET_KEY, match[1]);
    history.replaceState(null, "", window.location.pathname);
    return match[1];
  }
  return localStorage.getItem(SECRET_KEY);
}

type Tab = "decisions" | "tasks";

export function App() {
  const [secret] = useState<string | null>(resolveSecret);
  const [tab, setTab] = useState<Tab>("decisions");
  const [connected, setConnected] = useState(false);
  const [agentOnline, setAgentOnline] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [decisions, setDecisions] = useState<PendingDecision[]>([]);
  const [push, setPush] = useState<PushState>(pushState);
  const [detailSessionId, setDetailSessionId] = useState<string | null>(null);
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
      ];
      const fresh: PendingDecision[] = [];
      for (const [kind, list] of kinds) {
        for (const request of list ?? []) {
          fresh.push({ kind, id: request.id, request, arrivedAt: Date.now() });
        }
      }
      // Snapshot is authoritative: replaces the list (dismissals included).
      setDecisions(fresh);
    } catch {
      // agent offline — live events will catch us up later
    }
  }, []);

  useEffect(() => {
    if (!secret) return;
    const client = new RelayClient(secret, {
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
    });
    clientRef.current = client;
    client.connect();
    void resyncPush(client);
    return () => client.close();
  }, [secret, addDecision, removeDecision, refreshPending]);

  // Re-sync when the PWA returns to the foreground (mobile browsers freeze
  // sockets aggressively; the reconnect fires but the snapshot may be stale).
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible" && clientRef.current?.isAuthed) {
        void refreshPending(clientRef.current);
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [refreshPending]);

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
        <h1>Fleet 移动端</h1>
        <p>请在桌面端 Fleet 的「移动端」板块扫码打开本页面（链接里带配对密钥）。</p>
      </div>
    );
  }

  if (authError) {
    return (
      <div className={styles.gate}>
        <div className={styles.gateLogo}>F</div>
        <h1>配对失败</h1>
        <p>{authError}。密钥可能已被重置，请回到桌面端重新扫码。</p>
        <button
          className={styles.gateButton}
          onClick={() => {
            localStorage.removeItem(SECRET_KEY);
            location.reload();
          }}
        >
          清除本机密钥
        </button>
      </div>
    );
  }

  return (
    <div className={styles.app}>
      <header className={styles.header}>
        <span className={styles.title}>Fleet</span>
        <span
          className={styles.connDot}
          data-state={!connected ? "offline" : agentOnline ? "online" : "agent-offline"}
        />
        <span className={styles.connLabel}>
          {!connected ? "连接中…" : agentOnline ? "桌面端在线" : "桌面端离线"}
        </span>
      </header>

      {push !== "granted" && push !== "unsupported" && (
        <div className={styles.pushBanner}>
          {push === "ios-needs-a2hs" ? (
            <span>要接收通知，请先用 Safari 分享菜单「添加到主屏幕」，再从主屏幕打开。</span>
          ) : push === "denied" ? (
            <span>通知权限已被拒绝，请在系统设置中为本站点重新开启。</span>
          ) : (
            <>
              <span>开启通知，第一时间收到新决策卡。</span>
              <button className={styles.pushButton} onClick={handleEnablePush}>
                开启
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
          />
        ) : (
          <TasksView
            sessions={mergedSessions}
            client={clientRef.current}
            onOpenSession={(s) => setDetailSessionId(s.id)}
            onMarkRead={markRead}
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

      <nav className={styles.tabs}>
        <button
          className={styles.tabButton}
          data-active={tab === "decisions"}
          onClick={() => setTab("decisions")}
        >
          <span className={styles.tabIcon}>◈</span>
          决策
          {decisions.length > 0 && <span className={styles.badge}>{decisions.length}</span>}
        </button>
        <button
          className={styles.tabButton}
          data-active={tab === "tasks"}
          onClick={() => setTab("tasks")}
        >
          <span className={styles.tabIcon}>≣</span>
          任务
        </button>
      </nav>
    </div>
  );
}
