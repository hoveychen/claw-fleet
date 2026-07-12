import { useCallback, useState } from "react";
import type { RelayClient } from "../relay";
import type { SessionInfo, SessionStatus, TaskPlanDetail } from "../types";
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

interface Props {
  sessions: SessionInfo[];
  client: RelayClient | null;
}

export function TasksView({ sessions, client }: Props) {
  const [expanded, setExpanded] = useState<string | null>(null);
  const [plans, setPlans] = useState<Record<string, TaskPlanDetail[] | "loading" | "error">>({});

  const visible = sessions
    .filter((s) => !s.isSubagent)
    .sort((a, b) => b.lastActivityMs - a.lastActivityMs);

  const toggle = useCallback(
    (session: SessionInfo) => {
      const next = expanded === session.id ? null : session.id;
      setExpanded(next);
      if (next && client && plans[session.id] === undefined) {
        setPlans((p) => ({ ...p, [session.id]: "loading" }));
        client
          .request<TaskPlanDetail[]>("task_plans", {
            workspacePath: session.workspacePath,
            sessionId: session.id,
          })
          .then((detail) => setPlans((p) => ({ ...p, [session.id]: detail })))
          .catch(() => setPlans((p) => ({ ...p, [session.id]: "error" })));
      }
    },
    [expanded, client, plans],
  );

  if (visible.length === 0) {
    return <div className={styles.empty}>暂无会话（等待桌面端推送快照…）</div>;
  }

  return (
    <div className={styles.list}>
      {visible.map((s) => {
        const meta = statusMeta(s.status);
        const plan = s.taskPlan;
        const todos = s.todos;
        const todoTotal = todos ? todos.completed + todos.inProgress + todos.pending : 0;
        const detail = plans[s.id];
        return (
          <div key={s.id} className={styles.card} onClick={() => toggle(s)}>
            <div className={styles.cardHead}>
              <span className={styles.statusDot} data-tone={meta.tone} />
              <span className={styles.workspace}>{s.workspaceName}</span>
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
            {expanded === s.id && (
              <div className={styles.detail} onClick={(e) => e.stopPropagation()}>
                {detail === "loading" && <div className={styles.detailHint}>加载计划中…</div>}
                {detail === "error" && (
                  <div className={styles.detailHint}>计划加载失败（桌面端可能离线）</div>
                )}
                {Array.isArray(detail) && detail.length === 0 && (
                  <div className={styles.detailHint}>该会话没有 TASKS.md 计划</div>
                )}
                {Array.isArray(detail) &&
                  detail.map((p, i) => (
                    <div key={p.id ?? i} className={styles.plan}>
                      {p.title && <div className={styles.planTitle}>{p.title}</div>}
                      {p.items.map((item, j) => (
                        <div key={j} className={styles.planItem} data-done={item.done}>
                          <span className={styles.checkbox}>{item.done ? "✓" : ""}</span>
                          <span>{item.text}</span>
                        </div>
                      ))}
                    </div>
                  ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
