// Lazy-loading tab bodies for the session detail page: decision history,
// task plans, token breakdown, workflow runs, handoff chain. Each fetches on
// first open via its relay method and renders a compact mobile layout.

import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { RelayClient } from "../relay";
import type {
  DecisionHistoryRecord,
  HandoffChain,
  SessionInfo,
  TaskPlanDetail,
  TokenBreakdown,
  WorkflowTree,
} from "../types";
import styles from "./SessionDetailTabs.module.css";

/** One-shot fetch helper: "loading" → data | "error". */
function useRelayData<T>(
  client: RelayClient | null,
  method: string,
  params: Record<string, unknown>,
): T | "loading" | "error" {
  const [state, setState] = useState<T | "loading" | "error">("loading");
  const key = JSON.stringify(params);
  useEffect(() => {
    if (!client) {
      setState("error");
      return;
    }
    let cancelled = false;
    setState("loading");
    client
      .request<T>(method, JSON.parse(key) as Record<string, unknown>)
      .then((data) => {
        if (!cancelled) setState(data);
      })
      .catch(() => {
        if (!cancelled) setState("error");
      });
    return () => {
      cancelled = true;
    };
  }, [client, method, key]);
  return state;
}

function Hint({ children }: { children: React.ReactNode }) {
  return <div className={styles.hint}>{children}</div>;
}

function fmtDateTime(ts?: string | null): string {
  if (!ts) return "";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// ── 决策历史 ─────────────────────────────────────────────────────────────────

const KIND_LABEL: Record<string, string> = {
  elicitation: "问题请示",
  "plan-approval": "计划审批",
  "user-prompt": "用户输入",
  "fleet-ask": "决策卡",
};

const OUTCOME_LABEL: Record<string, string> = {
  answered: "已回答",
  declined: "已拒答",
  cancelled: "已取消",
  timeout: "超时",
  "heartbeat-lost": "面板掉线",
  approved: "已批准",
  "approved-with-edits": "批准（有编辑）",
  rejected: "已驳回",
};

function outcomeTone(outcome: string): string {
  if (["answered", "approved", "approved-with-edits"].includes(outcome)) return "good";
  if (["declined", "rejected", "cancelled"].includes(outcome)) return "bad";
  return "dim";
}

export function DecisionHistoryTab({
  session,
  client,
}: {
  session: SessionInfo;
  client: RelayClient | null;
}) {
  const data = useRelayData<DecisionHistoryRecord[]>(client, "session_decisions", {
    sessionId: session.id,
    jsonlPath: session.jsonlPath,
  });
  const [open, setOpen] = useState<Set<string>>(new Set());

  if (data === "loading") return <Hint>加载决策历史…</Hint>;
  if (data === "error") return <Hint>加载失败（桌面端可能离线）</Hint>;
  if (data.length === 0) return <Hint>该会话没有决策记录</Hint>;

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className={styles.stack}>
      {data.map((r) => {
        const expanded = open.has(r.id);
        const time = r.kind === "user-prompt" ? r.sentAt : r.resolvedAt;
        return (
          <div key={`${r.kind}:${r.id}`} className={styles.record} onClick={() => toggle(r.id)}>
            <div className={styles.recordHead}>
              <span className={styles.kindChip}>{KIND_LABEL[r.kind] ?? r.kind}</span>
              {r.kind !== "user-prompt" && (
                <span className={styles.outcomeChip} data-tone={outcomeTone(r.outcome)}>
                  {OUTCOME_LABEL[r.outcome] ?? r.outcome}
                </span>
              )}
              <span className={styles.recordTime}>{fmtDateTime(time)}</span>
            </div>
            <div className={styles.recordSummary} data-expanded={expanded}>
              {r.kind === "user-prompt" && (
                <>
                  {r.text}
                  {r.hasImage && <span className={styles.dimNote}>（含图片）</span>}
                </>
              )}
              {r.kind === "elicitation" && (r.questions[0]?.question ?? "")}
              {r.kind === "plan-approval" && r.planContent.slice(0, expanded ? undefined : 200)}
              {r.kind === "fleet-ask" && (r.questions[0]?.question ?? "")}
            </div>
            {expanded && r.kind === "elicitation" && (
              <div className={styles.recordBody} onClick={(e) => e.stopPropagation()}>
                {r.questions.map((q, i) => {
                  const picked = r.answers[q.question];
                  return (
                    <div key={i} className={styles.qaBlock}>
                      <div className={styles.qaQuestion}>{q.question}</div>
                      {q.options.map((o, j) => (
                        <div
                          key={j}
                          className={styles.qaOption}
                          data-picked={picked?.label === o.label}
                        >
                          {o.label}
                        </div>
                      ))}
                      {picked?.other && (
                        <div className={styles.qaOption} data-picked="true">
                          其他：{picked.label}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
            {expanded && r.kind === "plan-approval" && (
              <div className={styles.recordBody} onClick={(e) => e.stopPropagation()}>
                {r.editedPlan && (
                  <div className={styles.qaBlock}>
                    <div className={styles.qaQuestion}>用户编辑后的计划</div>
                    <div className={styles.preWrap}>{r.editedPlan}</div>
                  </div>
                )}
                {r.feedback && (
                  <div className={styles.qaBlock}>
                    <div className={styles.qaQuestion}>驳回意见</div>
                    <div className={styles.preWrap}>{r.feedback}</div>
                  </div>
                )}
              </div>
            )}
            {expanded && r.kind === "fleet-ask" && (
              <div className={styles.recordBody} onClick={(e) => e.stopPropagation()}>
                {r.questions.some((q) => q.html) && (
                  <div className={styles.dimNote}>[当时展示过 HTML 预览]</div>
                )}
                {Object.entries(r.answers).map(([k, v]) => (
                  <div key={k} className={styles.qaBlock}>
                    <div className={styles.qaQuestion}>{k}</div>
                    <div className={styles.preWrap}>{v}</div>
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

// ── 任务计划 ─────────────────────────────────────────────────────────────────

export function TaskPlansTab({
  session,
  client,
}: {
  session: SessionInfo;
  client: RelayClient | null;
}) {
  const data = useRelayData<TaskPlanDetail[]>(client, "task_plans", {
    workspacePath: session.workspacePath,
    sessionId: session.id,
  });

  if (data === "loading") return <Hint>加载计划中…</Hint>;
  if (data === "error") return <Hint>加载失败（桌面端可能离线）</Hint>;
  if (data.length === 0) return <Hint>该会话没有 TASKS.md 计划</Hint>;

  const current = session.taskPlan?.currentTask;
  return (
    <div className={styles.stack}>
      {data.map((p, i) => (
        <div key={p.id ?? i} className={styles.planCard}>
          {p.title && <div className={styles.planTitle}>{p.title}</div>}
          {p.items.map((item, j) => {
            const isCurrent = Boolean(
              current && !item.done && item.text.includes(current),
            );
            return (
              <div key={j} className={styles.planItem} data-done={item.done} data-current={isCurrent}>
                <span className={styles.checkbox}>{item.done ? "✓" : ""}</span>
                <span>{item.text}</span>
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}

// ── Token ────────────────────────────────────────────────────────────────────

function fmtTokens(n?: number): string {
  if (!n) return "0";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

export function TokenTab({
  session,
  client,
}: {
  session: SessionInfo;
  client: RelayClient | null;
}) {
  const data = useRelayData<TokenBreakdown>(client, "token_breakdown", {
    path: session.jsonlPath,
    projectRoot: session.workspacePath,
  });

  if (data === "loading") return <Hint>分析 token 用量…</Hint>;
  if (data === "error") return <Hint>分析失败（桌面端可能离线）</Hint>;

  const u = data.totalsUsage ?? {};
  const rows: Array<[string, string]> = [
    ["输入 tokens", fmtTokens(u.inputTokens)],
    ["输出 tokens", fmtTokens(u.outputTokens)],
    ["缓存写入", fmtTokens(u.cacheCreationTokens)],
    ["缓存读取", fmtTokens(u.cacheReadTokens)],
  ];
  return (
    <div className={styles.stack}>
      <div className={styles.statGrid}>
        {rows.map(([label, value]) => (
          <div key={label} className={styles.statTile}>
            <div className={styles.statValue}>{value}</div>
            <div className={styles.statLabel}>{label}</div>
          </div>
        ))}
      </div>
      {data.totalsEstimatedCostUsd != null && (
        <div className={styles.costLine}>
          估算成本 <strong>${data.totalsEstimatedCostUsd.toFixed(2)}</strong>
          {(data.subagents?.length ?? 0) > 0 && (
            <span className={styles.dimNote}>（含 {data.subagents!.length} 个子 agent）</span>
          )}
        </div>
      )}
    </div>
  );
}

// ── Workflow ─────────────────────────────────────────────────────────────────

const AGENT_STATUS_LABEL: Record<string, string> = {
  running: "运行中",
  done: "完成",
  error: "出错",
  pending: "排队",
};

export function WorkflowTab({
  session,
  client,
}: {
  session: SessionInfo;
  client: RelayClient | null;
}) {
  const data = useRelayData<WorkflowTree[]>(client, "workflow_trees", {
    path: session.jsonlPath,
  });

  if (data === "loading") return <Hint>加载 workflow…</Hint>;
  if (data === "error") return <Hint>加载失败（桌面端可能离线）</Hint>;
  if (data.length === 0) return <Hint>该会话没有 workflow 运行</Hint>;

  return (
    <div className={styles.stack}>
      {data.map((t) => (
        <div key={t.runId} className={styles.planCard}>
          <div className={styles.planTitle}>{t.name || t.runId}</div>
          {t.description && <div className={styles.dimNote}>{t.description}</div>}
          {t.agents.map((a, i) => (
            <div key={a.agentId ?? i} className={styles.wfAgent}>
              <span className={styles.wfStatus} data-status={a.status}>
                {AGENT_STATUS_LABEL[a.status ?? ""] ?? a.status ?? "?"}
              </span>
              <span className={styles.wfLabel}>{a.label || a.prompt || a.agentId}</span>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}

// ── Handoff 接力链 ───────────────────────────────────────────────────────────

export function HandoffTab({
  session,
  client,
}: {
  session: SessionInfo;
  client: RelayClient | null;
}) {
  const data = useRelayData<HandoffChain | null>(client, "handoff_chain", {
    sessionId: session.id,
  });

  if (data === "loading") return <Hint>加载接力链…</Hint>;
  if (data === "error") return <Hint>加载失败（桌面端可能离线）</Hint>;
  if (!data) return <Hint>该会话不在任何接力链上</Hint>;

  const hops: string[] = [];
  for (const l of data.links) {
    if (hops[hops.length - 1] !== l.fromSessionId) hops.push(l.fromSessionId);
    hops.push(l.toSessionId);
  }

  return (
    <div className={styles.stack}>
      <div className={styles.dimNote}>
        接力 {hops.length} 棒
        {hops.includes(session.id) && ` · 当前第 ${hops.indexOf(session.id) + 1} 棒`}
      </div>
      {data.links.map((l, i) => (
        <div key={i} className={styles.planCard}>
          <div className={styles.hopHead}>
            <span className={styles.hopBadge}>第 {i + 1} → {i + 2} 棒</span>
            <span className={styles.recordTime}>
              {new Date(l.handedAt).toLocaleString("zh-CN", {
                month: "2-digit",
                day: "2-digit",
                hour: "2-digit",
                minute: "2-digit",
              })}
            </span>
          </div>
          {(l.planId || l.nextTask) && (
            <div className={styles.dimNote}>
              {l.planId && `计划 ${l.planId}`}
              {l.nextTask && ` · 下一步 ${l.nextTask}`}
            </div>
          )}
          <div className={styles.markdown}>
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{l.note}</ReactMarkdown>
          </div>
        </div>
      ))}
    </div>
  );
}
