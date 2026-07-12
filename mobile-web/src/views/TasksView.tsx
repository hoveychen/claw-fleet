import { useCallback, useMemo, useState, type ReactNode } from "react";
import { t } from "../i18n";
import type { RelayClient } from "../relay";
import type { SessionInfo, SessionMark, SessionStatus } from "../types";
import { isFleetOwnedEntrypoint, isSessionUnread } from "../types";
import { useRelaySearch } from "../useRelaySearch";
import { NewSessionSheet } from "./Composer";
import styles from "./TasksView.module.css";

const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];
const LIVE: SessionStatus[] = [...WORKING, "waitingInput", "active", "rateLimited"];

/** Dot tone for the row status accent — mirrors the desktop launchpad's
 *  `rowBarColor`: a colour only for live/waiting rows, `null` (no dot) for
 *  idle. The status is read off the dot, so there's no separate text pill. */
function statusTone(status: SessionStatus): string | null {
  if (WORKING.includes(status)) return "working";
  if (status === "waitingInput") return "waiting";
  if (status === "active") return "active";
  if (status === "rateLimited") return "error";
  return null;
}

function timeAgo(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("刚刚");
  if (diff < 3_600_000) return t("{0} 分钟前", Math.floor(diff / 60_000));
  if (diff < 86_400_000) return t("{0} 小时前", Math.floor(diff / 3_600_000));
  return t("{0} 天前", Math.floor(diff / 86_400_000));
}

/** Elapsed run time = now − session start; shown only for live rows, matching
 *  the desktop launchpad's `formatRunning`. Single-unit, counts up from start. */
function formatRunning(ms: number): string {
  const diff = Math.max(0, Date.now() - ms);
  if (diff < 60_000) return t("运行 {0} 秒", Math.floor(diff / 1_000));
  if (diff < 3_600_000) return t("运行 {0} 分", Math.floor(diff / 60_000));
  if (diff < 86_400_000) return t("运行 {0} 时", Math.floor(diff / 3_600_000));
  return t("运行 {0} 天", Math.floor(diff / 86_400_000));
}

type StopMode = "interrupt" | "stop" | "spent";

/** Same escalation as the desktop StopControl: interrupt only for Fleet-owned
 *  sessions with a precise pid mid-turn; otherwise kill; no pid → dead. */
function stopMode(s: SessionInfo): StopMode {
  if (s.pid == null) return "spent";
  if (isFleetOwnedEntrypoint(s.entrypoint) && s.pidPrecise && WORKING.includes(s.status)) {
    return "interrupt";
  }
  return "stop";
}

function canControl(s: SessionInfo): boolean {
  return !s.isSubagent && s.agentSource !== "cursor";
}

type MarkFilter = "all" | "pending" | "done";

/** Same bucketing as the desktop launchpad: an unmarked session still needs
 *  attention, so it collapses into "pending" — only an explicit done leaves. */
function markBucket(s: SessionInfo): SessionMark {
  return s.userMark === "done" ? "done" : "pending";
}

/** FTS5 snippets arrive with literal `<mark>…</mark>` markers (see the desktop
 *  HistoryView). Split them into React nodes instead of trusting the transcript
 *  text as HTML. */
function renderSnippet(snippet: string): ReactNode[] {
  return snippet.split("<mark>").flatMap((chunk, i) => {
    if (i === 0) return [chunk];
    const end = chunk.indexOf("</mark>");
    if (end === -1) return [chunk];
    return [<mark key={i}>{chunk.slice(0, end)}</mark>, chunk.slice(end + "</mark>".length)];
  });
}

// ── Inline icons (mobile-web has no lucide dep; match the desktop glyphs) ─────
const IconSearch = () => (
  <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
    <circle cx="7" cy="7" r="4.5" />
    <line x1="10.5" y1="10.5" x2="14" y2="14" />
  </svg>
);
const IconFolder = () => (
  <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round">
    <path d="M1.5 4a1.5 1.5 0 0 1 1.5-1.5h3L7.5 4H13a1.5 1.5 0 0 1 1.5 1.5v7A1.5 1.5 0 0 1 13 14H3a1.5 1.5 0 0 1-1.5-1.5V4Z" />
  </svg>
);
const IconClock = () => (
  <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
    <circle cx="8" cy="8" r="6" />
    <path d="M8 4.5V8l2.5 1.5" />
  </svg>
);
const IconRelay = () => (
  <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
    <circle cx="3.5" cy="3.5" r="2" />
    <circle cx="12.5" cy="12.5" r="2" />
    <circle cx="12.5" cy="3.5" r="2" />
    <path d="M5.5 3.5h5M3.5 5.5v4.5a2 2 0 0 0 2 2h5" />
  </svg>
);
const IconCircle = () => (
  <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.8">
    <circle cx="8" cy="8" r="6.2" />
  </svg>
);
const IconCheckCircle = () => (
  <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
    <circle cx="8" cy="8" r="6.2" />
    <path d="M5.3 8.1l1.9 1.9 3.6-3.9" />
  </svg>
);

interface Props {
  sessions: SessionInfo[];
  client: RelayClient | null;
  onOpenSession: (session: SessionInfo) => void;
  onMarkRead: (sessions: SessionInfo[]) => void;
}

export function TasksView({ sessions, client, onOpenSession, onMarkRead }: Props) {
  const [search, setSearch] = useState("");
  const [workspace, setWorkspace] = useState<string>("");
  const [activeOnly, setActiveOnly] = useState(false);
  const [markFilter, setMarkFilter] = useState<MarkFilter>("all");
  const [busyOp, setBusyOp] = useState<string | null>(null);
  const [showNewSession, setShowNewSession] = useState(false);
  // Optimistic mark overrides, dropped once the server snapshot catches up.
  const [markOverride, setMarkOverride] = useState<Record<string, SessionMark | null>>({});

  // Full-text search over the relay — same FTS the desktop launchpad uses.
  const { searching, ftsMatchPaths, snippetByPath } = useRelaySearch(client, search);

  // Scope to Fleet-launched sessions (新会话 / handoff relay), exactly like the
  // desktop 任务 launchpad (`adhocSessions`). Externally-started transcripts
  // (VS Code / bare CLI) are deliberately out of this list.
  const all = useMemo(
    () =>
      sessions
        .filter((s) => !s.isSubagent && isFleetOwnedEntrypoint(s.entrypoint))
        .map((s) => {
          const o = markOverride[s.id];
          return o !== undefined && o !== (s.userMark ?? null) ? { ...s, userMark: o } : s;
        })
        .sort((a, b) => b.lastActivityMs - a.lastActivityMs),
    [sessions, markOverride],
  );

  const workspaces = useMemo(() => {
    const names = new Map<string, string>();
    for (const s of all) names.set(s.workspacePath, s.workspaceName);
    return [...names.entries()].sort((a, b) => a[1].localeCompare(b[1]));
  }, [all]);

  const activeCount = useMemo(() => all.filter((s) => LIVE.includes(s.status)).length, [all]);

  // Everything except the mark filter — the segment counts are taken over this
  // set so each count reflects how many rows its segment would reveal under the
  // current workspace / query / active filters (mirrors the desktop `preMark`).
  const preMark = useMemo(() => {
    const q = search.trim().toLowerCase();
    return all.filter((s) => {
      if (workspace && s.workspacePath !== workspace) return false;
      if (activeOnly && !LIVE.includes(s.status)) return false;
      if (q) {
        const clientMatch =
          `${s.aiTitle ?? ""} ${s.slug ?? ""} ${s.lastMessagePreview ?? ""} ${s.workspaceName}`
            .toLowerCase()
            .includes(q) ||
          // The attributed TASKS.md plan — id / title / current P-task, so a
          // handoff relay chain is reachable by the plan name too.
          (s.taskPlan?.planId?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentPlan?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentTask?.toLowerCase().includes(q) ?? false);
        if (!clientMatch && !ftsMatchPaths.has(s.jsonlPath)) return false;
      }
      return true;
    });
  }, [all, search, workspace, activeOnly, ftsMatchPaths]);

  const counts = useMemo(() => {
    let pending = 0;
    let done = 0;
    for (const s of preMark) {
      if (markBucket(s) === "done") done++;
      else pending++;
    }
    return { all: preMark.length, pending, done };
  }, [preMark]);

  const visible = useMemo(
    () => preMark.filter((s) => markFilter === "all" || markBucket(s) === markFilter),
    [preMark, markFilter],
  );

  const unreadCount = useMemo(() => visible.filter(isSessionUnread).length, [visible]);

  const setMark = useCallback(
    (s: SessionInfo, mark: SessionMark | null) => {
      if (!client) return;
      setMarkOverride((prev) => ({ ...prev, [s.id]: mark }));
      client
        .request("session_mark", {
          sessionId: s.id,
          workspacePath: s.workspacePath,
          ...(mark ? { mark } : {}),
        })
        .catch(() => {
          // roll back the optimistic flip on failure
          setMarkOverride((prev) => {
            const next = { ...prev };
            delete next[s.id];
            return next;
          });
        });
    },
    [client],
  );

  const handleStop = useCallback(
    async (s: SessionInfo) => {
      if (!client || busyOp) return;
      const mode = stopMode(s);
      if (mode === "spent") return;
      setBusyOp(s.id);
      try {
        if (mode === "interrupt") {
          await client.request("interrupt", { pid: s.pid });
        } else if (s.pidPrecise) {
          if (!window.confirm(t("确定停止「{0}」的这个会话吗？", s.workspaceName))) return;
          await client.request("stop", { pid: s.pid });
        } else {
          if (
            !window.confirm(
              t("无法精确定位进程，将停止「{0}」目录下的所有会话，确定吗？", s.workspaceName),
            )
          )
            return;
          await client.request("stop_workspace", { workspacePath: s.workspacePath });
        }
      } catch (e) {
        window.alert(e instanceof Error ? e.message : t("操作失败"));
      } finally {
        setBusyOp(null);
      }
    },
    [client, busyOp],
  );

  if (all.length === 0) {
    return <div className={styles.empty}>{t("暂无会话（等待桌面端推送快照…）")}</div>;
  }

  const segLabels: Record<MarkFilter, string> = {
    all: t("全部"),
    pending: t("进行中"),
    done: t("已完成"),
  };

  return (
    <div className={styles.wrapper}>
      <div className={styles.filterBar}>
        <div className={styles.searchWrap}>
          <span className={styles.searchIcon}>
            <IconSearch />
          </span>
          <input
            className={styles.search}
            type="search"
            placeholder={t("搜索标题、计划、全文…")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          {searching && <span className={styles.searchSpinner} />}
        </div>
        <div className={styles.filterRow}>
          <select
            className={styles.workspaceSelect}
            value={workspace}
            onChange={(e) => setWorkspace(e.target.value)}
          >
            <option value="">{t("全部目录")}</option>
            {workspaces.map(([path, name]) => (
              <option key={path} value={path}>
                {name}
              </option>
            ))}
          </select>
          <button
            className={styles.filterToggle}
            data-active={activeOnly}
            onClick={() => setActiveOnly((v) => !v)}
          >
            <span className={styles.activeDot} />
            {t("仅活跃")}
            <span className={styles.activeCount}>{activeCount}</span>
          </button>
          <button className={styles.newSession} onClick={() => setShowNewSession(true)}>
            {t("＋ 新会话")}
          </button>
          {unreadCount > 0 && (
            <button
              className={styles.readAll}
              onClick={() => onMarkRead(visible.filter(isSessionUnread))}
            >
              {t("全部已读 ({0})", unreadCount)}
            </button>
          )}
        </div>
        <div className={styles.segment}>
          {(["all", "pending", "done"] as MarkFilter[]).map((key) => (
            <button
              key={key}
              className={styles.segmentButton}
              data-active={markFilter === key}
              onClick={() => setMarkFilter(key)}
              aria-label={`${segLabels[key]} (${counts[key]})`}
              title={segLabels[key]}
            >
              {key === "all" && <span>{segLabels.all}</span>}
              {key === "pending" && <IconCircle />}
              {key === "done" && <IconCheckCircle />}
              <span className={styles.segmentCount}>{counts[key]}</span>
            </button>
          ))}
        </div>
      </div>

      {visible.length === 0 && <div className={styles.empty}>{t("没有匹配的会话")}</div>}

      <div className={styles.list}>
        {visible.map((s) => {
          const tone = statusTone(s.status);
          const mode = stopMode(s);
          const unread = isSessionUnread(s);
          const isDone = s.userMark === "done";
          const title = s.aiTitle || s.slug || s.lastMessagePreview || t("（无标题）");
          const snippet = search.trim().length >= 2 ? snippetByPath.get(s.jsonlPath) : undefined;
          const live = LIVE.includes(s.status);
          return (
            <div key={s.id} className={styles.card} onClick={() => onOpenSession(s)}>
              <div className={styles.cardHead}>
                {tone && <span className={styles.statusDot} data-tone={tone} />}
                <span className={styles.title}>{title}</span>
                {unread && <span className={styles.unreadDot} />}
                <span className={styles.time}>{timeAgo(s.lastActivityMs)}</span>
              </div>
              <div className={styles.metaRow}>
                <span className={styles.project}>
                  <IconFolder />
                  {s.workspaceName}
                </span>
                {s.handoff && (
                  <span className={styles.handoff}>
                    <IconRelay />
                    {s.handoff.hop}/{s.handoff.chainLen}
                  </span>
                )}
                {live && (
                  <span className={styles.runtime} data-tone={tone ?? undefined}>
                    <IconClock />
                    {formatRunning(s.createdAtMs)}
                  </span>
                )}
              </div>
              {snippet ? (
                <div className={styles.snippet}>{renderSnippet(snippet)}</div>
              ) : (
                s.lastMessagePreview && <div className={styles.preview}>{s.lastMessagePreview}</div>
              )}
              <div className={styles.opsRow} onClick={(e) => e.stopPropagation()}>
                <button
                  className={styles.markToggle}
                  data-done={isDone}
                  onClick={() => setMark(s, isDone ? null : "done")}
                  aria-pressed={isDone}
                  title={isDone ? t("已完成 — 点击改回进行中") : t("进行中 — 点击标为已完成")}
                >
                  {isDone ? <IconCheckCircle /> : <IconCircle />}
                </button>
                <span className={styles.opsSpacer} />
                {canControl(s) && mode !== "spent" && (
                  <button
                    className={styles.stopButton}
                    data-mode={mode}
                    disabled={busyOp === s.id}
                    onClick={() => void handleStop(s)}
                  >
                    {busyOp === s.id ? "…" : `◼ ${mode === "interrupt" ? t("中断") : t("停止")}`}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {showNewSession && (
        <NewSessionSheet sessions={sessions} client={client} onClose={() => setShowNewSession(false)} />
      )}
    </div>
  );
}
