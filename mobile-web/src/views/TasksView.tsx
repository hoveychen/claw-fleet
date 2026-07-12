import { useCallback, useMemo, useState } from "react";
import type { RelayClient } from "../relay";
import type { SessionInfo, SessionMark, SessionStatus } from "../types";
import { isFleetOwnedEntrypoint, isSessionUnread } from "../types";
import { NewSessionSheet } from "./Composer";
import styles from "./TasksView.module.css";

const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];

function statusMeta(status: SessionStatus): { label: string; tone: string } {
  if (WORKING.includes(status)) return { label: "工作中", tone: "working" };
  switch (status) {
    case "waitingInput":
      return { label: "等待输入", tone: "waiting" };
    case "active":
      return { label: "活跃", tone: "active" };
    case "rateLimited":
      return { label: "限流中", tone: "error" };
    default:
      return { label: "空闲", tone: "idle" };
  }
}

function timeAgo(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  return `${Math.floor(diff / 86_400_000)} 天前`;
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

const LIVE: SessionStatus[] = [...WORKING, "waitingInput", "active", "rateLimited"];

type MarkFilter = "all" | "pending" | "done";

/** Same bucketing as the desktop launchpad: an unmarked session still needs
 *  attention, so it collapses into "pending" — only an explicit done leaves. */
function markBucket(s: SessionInfo): SessionMark {
  return s.userMark === "done" ? "done" : "pending";
}

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

  const all = useMemo(
    () =>
      sessions
        .filter((s) => !s.isSubagent)
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

  const counts = useMemo(() => {
    let pending = 0;
    let done = 0;
    for (const s of all) {
      if (markBucket(s) === "done") done++;
      else pending++;
    }
    return { pending, done };
  }, [all]);

  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    return all.filter((s) => {
      if (workspace && s.workspacePath !== workspace) return false;
      if (activeOnly && !LIVE.includes(s.status)) return false;
      if (markFilter !== "all" && markBucket(s) !== markFilter) return false;
      if (q) {
        const hay = `${s.aiTitle ?? ""} ${s.slug ?? ""} ${s.lastMessagePreview ?? ""} ${s.workspaceName}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [all, search, workspace, activeOnly, markFilter]);

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
          if (!window.confirm(`确定停止「${s.workspaceName}」的这个会话吗？`)) return;
          await client.request("stop", { pid: s.pid });
        } else {
          if (
            !window.confirm(
              `无法精确定位进程，将停止「${s.workspaceName}」目录下的所有会话，确定吗？`,
            )
          )
            return;
          await client.request("stop_workspace", { workspacePath: s.workspacePath });
        }
      } catch (e) {
        window.alert(e instanceof Error ? e.message : "操作失败");
      } finally {
        setBusyOp(null);
      }
    },
    [client, busyOp],
  );

  if (all.length === 0) {
    return <div className={styles.empty}>暂无会话（等待桌面端推送快照…）</div>;
  }

  return (
    <div className={styles.wrapper}>
      <div className={styles.filterBar}>
        <input
          className={styles.search}
          type="search"
          placeholder="搜索标题 / 摘要…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <div className={styles.filterRow}>
          <select
            className={styles.workspaceSelect}
            value={workspace}
            onChange={(e) => setWorkspace(e.target.value)}
          >
            <option value="">全部目录</option>
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
            仅活跃
          </button>
          <button className={styles.newSession} onClick={() => setShowNewSession(true)}>
            ＋ 新会话
          </button>
          {unreadCount > 0 && (
            <button className={styles.readAll} onClick={() => onMarkRead(visible.filter(isSessionUnread))}>
              全部已读 ({unreadCount})
            </button>
          )}
        </div>
        <div className={styles.segment}>
          {(
            [
              ["all", `全部`],
              ["pending", `进行中${counts.pending ? ` ${counts.pending}` : ""}`],
              ["done", `已完成${counts.done ? ` ${counts.done}` : ""}`],
            ] as Array<[MarkFilter, string]>
          ).map(([key, label]) => (
            <button
              key={key}
              className={styles.segmentButton}
              data-active={markFilter === key}
              onClick={() => setMarkFilter(key)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {visible.length === 0 && <div className={styles.empty}>没有匹配的会话</div>}

      <div className={styles.list}>
        {visible.map((s) => {
          const meta = statusMeta(s.status);
          const plan = s.taskPlan;
          const todos = s.todos;
          const todoTotal = todos ? todos.completed + todos.inProgress + todos.pending : 0;
          const mode = stopMode(s);
          const unread = isSessionUnread(s);
          return (
            <div key={s.id} className={styles.card} onClick={() => onOpenSession(s)}>
              <div className={styles.cardHead}>
                <span className={styles.statusDot} data-tone={meta.tone} />
                <span className={styles.workspace}>{s.workspaceName}</span>
                {unread && <span className={styles.unreadDot} />}
                <span className={styles.time}>{timeAgo(s.lastActivityMs)}</span>
              </div>
              {(s.aiTitle || s.slug) && <div className={styles.aiTitle}>{s.aiTitle || s.slug}</div>}
              {s.lastMessagePreview && (
                <div className={styles.preview}>{s.lastMessagePreview}</div>
              )}
              <div className={styles.metaRow}>
                <span className={styles.statusLabel} data-tone={meta.tone}>
                  {meta.label}
                </span>
                {plan && plan.total > 0 && (
                  <span className={styles.planChip}>
                    计划 {plan.done}/{plan.total}
                    {plan.currentTask ? ` · ${plan.currentTask}` : ""}
                  </span>
                )}
                {todos && todoTotal > 0 && (
                  <span className={styles.todoChip}>
                    待办 {todos.completed}/{todoTotal}
                  </span>
                )}
              </div>
              {plan && plan.total > 0 && (
                <div className={styles.progress}>
                  <div
                    className={styles.progressFill}
                    style={{ width: `${Math.round((plan.done / plan.total) * 100)}%` }}
                  />
                </div>
              )}
              <div className={styles.opsRow} onClick={(e) => e.stopPropagation()}>
                <button
                  className={styles.markButton}
                  data-active={s.userMark === "pending"}
                  onClick={() => setMark(s, s.userMark === "pending" ? null : "pending")}
                >
                  待处理
                </button>
                <button
                  className={styles.markButton}
                  data-active={s.userMark === "done"}
                  onClick={() => setMark(s, s.userMark === "done" ? null : "done")}
                >
                  已完成
                </button>
                <span className={styles.opsSpacer} />
                {canControl(s) && mode !== "spent" && (
                  <button
                    className={styles.stopButton}
                    data-mode={mode}
                    disabled={busyOp === s.id}
                    onClick={() => void handleStop(s)}
                  >
                    {busyOp === s.id ? "…" : mode === "interrupt" ? "◼ 中断" : "◼ 停止"}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {showNewSession && (
        <NewSessionSheet
          sessions={sessions}
          client={client}
          onClose={() => setShowNewSession(false)}
        />
      )}
    </div>
  );
}
