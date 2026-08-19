// Lazy-loading tab bodies for the session detail page: decision history,
// task plans, token breakdown, workflow runs, handoff chain. Each fetches on
// first open via its relay method and renders a compact mobile layout.

import { useEffect, useState } from "react";
import { Check, CheckCircle2, ChevronRight, ListTodo, Waypoints, Workflow } from "lucide-react";
import { EmptyState } from "./EmptyState";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { dateLocale, t } from "../i18n";
import type { RelayClient } from "../relay";
import type {
  DecisionHistoryRecord,
  HandoffChain,
  SessionInfo,
  TaskPlanDetail,
  TokenBreakdown,
  WorkflowTree,
} from "../types";
import { useAgentNav } from "./AgentNavContext";
import { AttachmentThumbs } from "./AttachmentThumb";
import { splitAnswerAttachments } from "../userAttachments";
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
  return d.toLocaleString(dateLocale(), {
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
  if (["timeout", "heartbeat-lost"].includes(outcome)) return "warn";
  return "dim";
}

/** Sort/display by request time (sent time for user prompts) — matches the
 *  desktop DecisionHistory, which reads chronologically oldest-first. */
function recordTime(r: DecisionHistoryRecord): string {
  return r.kind === "user-prompt" ? r.sentAt : r.requestedAt;
}

/** One-line collapsed preview: whitespace-collapsed and rendered raw (markdown
 *  in a clamped one-liner reads worse than plain text). */
function recordSummary(r: DecisionHistoryRecord): string {
  if (r.kind === "elicitation" || r.kind === "fleet-ask")
    return (r.questions[0]?.question ?? "").replace(/\s+/g, " ").trim();
  if (r.kind === "user-prompt") return r.text.replace(/\s+/g, " ").trim();
  return r.aiTitle ?? r.workspaceName ?? t("计划审批");
}

// Markdown for decision bodies: the shared full chain (GFM + CJK bold + KaTeX +
// mermaid + sanitize), same as the wiki/message views. Links are rendered inert
// (mobile has no in-app path navigation in this tab). The inline variant unwraps
// <p> so it can sit inside the option label/description <span>s.
const mdLink: Components["a"] = ({ children }) => (
  <span className={styles.mdLink}>{children}</span>
);
const MD_BLOCK: Components = { a: mdLink };
const MD_INLINE: Components = { a: mdLink, p: ({ children }) => <>{children}</> };

function Md({ text, inline }: { text: string; inline?: boolean }) {
  return (
    <ReactMarkdown
      remarkPlugins={mdRemarkPlugins}
      rehypePlugins={mdRehypePlugins}
      components={inline ? MD_INLINE : MD_BLOCK}
    >
      {text}
    </ReactMarkdown>
  );
}

/** Option row with a ✓/○/▸ marker + markdown label/description, mirroring the
 *  desktop DecisionHistory option layout. */
function OptionRow({
  label,
  description,
  selected,
  marker,
}: {
  label: string;
  description?: string | null;
  selected: boolean;
  marker?: string;
}) {
  return (
    <div className={styles.qaOption} data-picked={selected}>
      <span className={styles.qaOptionLabel}>
        <span className={styles.qaOptionMarker}>{marker ?? (selected ? "✓" : "○")}</span>
        <Md text={label} inline />
      </span>
      {description && (
        <span className={styles.qaOptionDesc}>
          <Md text={description} inline />
        </span>
      )}
    </div>
  );
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

  if (data === "loading") return <Hint>{t("加载决策历史…")}</Hint>;
  if (data === "error") return <Hint>{t("加载失败（桌面端可能离线）")}</Hint>;
  if (data.length === 0)
    return <EmptyState compact icon={CheckCircle2} title={t("该会话没有决策记录")} />;

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  // Oldest-first so the list reads chronologically as the session evolved.
  const ordered = [...data].sort((a, b) => recordTime(a).localeCompare(recordTime(b)));

  return (
    <div className={styles.stack}>
      {ordered.map((r) => {
        const expanded = open.has(r.id);
        return (
          <div key={`${r.kind}:${r.id}`} className={styles.record} onClick={() => toggle(r.id)}>
            <div className={styles.recordHead}>
              <span className={styles.kindChip} data-kind={r.kind}>
                {t(KIND_LABEL[r.kind] ?? r.kind)}
              </span>
              {r.kind !== "user-prompt" && (
                <span className={styles.outcomeChip} data-tone={outcomeTone(r.outcome)}>
                  {t(OUTCOME_LABEL[r.outcome] ?? r.outcome)}
                </span>
              )}
              <span className={styles.recordTime}>{fmtDateTime(recordTime(r))}</span>
            </div>
            <div className={styles.recordSummary} data-expanded={expanded}>
              {recordSummary(r)}
              {r.kind === "user-prompt" && r.hasImage && (
                <span className={styles.dimNote}> {t("（含图片）")}</span>
              )}
            </div>

            {expanded && r.kind === "elicitation" && (
              <div className={styles.recordBody} onClick={(e) => e.stopPropagation()}>
                {r.questions.map((q, i) => {
                  const picked = r.answers[q.question];
                  const selectedLabels =
                    picked && !picked.other
                      ? picked.label.split(",").map((s) => s.trim())
                      : [];
                  return (
                    <div key={i} className={styles.qaBlock}>
                      <div className={styles.qaQuestion}>
                        <Md text={q.question} />
                      </div>
                      {q.options.map((o, j) => (
                        <OptionRow
                          key={j}
                          label={o.label}
                          description={o.description}
                          selected={selectedLabels.includes(o.label)}
                        />
                      ))}
                      {picked?.other && (
                        <OptionRow label={t("其他")} description={picked.label} selected marker="✓" />
                      )}
                    </div>
                  );
                })}
              </div>
            )}

            {expanded && r.kind === "plan-approval" && (
              <div className={styles.recordBody} onClick={(e) => e.stopPropagation()}>
                <div className={styles.planBody}>
                  <Md text={r.planContent} />
                </div>
                {r.editedPlan && (
                  <div className={styles.qaBlock}>
                    <div className={styles.qaQuestion}>{t("用户编辑后的计划")}</div>
                    <div className={styles.planBody}>
                      <Md text={r.editedPlan} />
                    </div>
                  </div>
                )}
                {r.feedback && (
                  <div className={styles.qaBlock}>
                    <div className={styles.qaQuestion}>{t("驳回意见")}</div>
                    <div className={styles.feedback}>
                      <Md text={r.feedback} />
                    </div>
                  </div>
                )}
              </div>
            )}

            {expanded && r.kind === "user-prompt" && (
              <div className={styles.recordBody} onClick={(e) => e.stopPropagation()}>
                <pre className={styles.preWrap}>{r.text}</pre>
              </div>
            )}

            {expanded && r.kind === "fleet-ask" && (
              <div className={styles.recordBody} onClick={(e) => e.stopPropagation()}>
                {r.questions.map((q, i) => {
                  // An answer can carry `@/path` mentions for the files the user
                  // attached. They are part of the answer string, so they have to
                  // come off before the label matching below — and they are what
                  // the thumbnails at the end of the block render.
                  const { core: raw, attachments } = splitAnswerAttachments(
                    r.answers[q.question] ?? "",
                  );
                  const opts = q.options ?? [];
                  const fields = q.formFields ?? [];
                  const selectedLabels = raw
                    .split(",")
                    .map((s) => s.trim())
                    .filter(Boolean);
                  const matched = opts.some((o) => selectedLabels.includes(o.label));
                  const isOther = !matched && raw.length > 0 && opts.length > 0;
                  return (
                    <div key={i} className={styles.qaBlock}>
                      <div className={styles.qaQuestion}>
                        <Md text={q.question} />
                      </div>
                      {q.images && q.images.length > 0 ? (
                        <div className={styles.dimNote}>{t("[当时展示过图片预览]")}</div>
                      ) : (
                        q.html && (
                          <div className={styles.dimNote}>{t("[当时展示过 HTML 预览]")}</div>
                        )
                      )}
                      {opts.map((o, j) => (
                        <OptionRow
                          key={j}
                          label={o.label}
                          description={o.description}
                          selected={selectedLabels.includes(o.label)}
                        />
                      ))}
                      {isOther && (
                        <OptionRow label={t("其他")} description={raw} selected marker="✓" />
                      )}
                      {fields.map((f, fi) => {
                        const v = r.answers[f.name];
                        if (v === undefined || v === "") return null;
                        return (
                          <OptionRow
                            key={`f-${fi}`}
                            label={f.label || f.name}
                            description={v}
                            selected
                            marker="▸"
                          />
                        );
                      })}
                      {opts.length === 0 && fields.length === 0 && raw && (
                        <div className={styles.preWrap}>{raw}</div>
                      )}
                      <AttachmentThumbs paths={attachments} client={client} />
                    </div>
                  );
                })}
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

  if (data === "loading") return <Hint>{t("加载计划中…")}</Hint>;
  if (data === "error") return <Hint>{t("加载失败（桌面端可能离线）")}</Hint>;
  if (data.length === 0)
    return <EmptyState compact icon={ListTodo} title={t("该会话没有 TASKS.md 计划")} />;

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
                <span className={styles.checkbox}>{item.done ? <Check size={11} /> : ""}</span>
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

  if (data === "loading") return <Hint>{t("分析 token 用量…")}</Hint>;
  if (data === "error") return <Hint>{t("分析失败（桌面端可能离线）")}</Hint>;

  const u = data.totalsUsage ?? {};
  const rows: Array<[string, string]> = [
    [t("输入 tokens"), fmtTokens(u.inputTokens)],
    [t("输出 tokens"), fmtTokens(u.outputTokens)],
    [t("缓存写入"), fmtTokens(u.cacheCreationTokens)],
    [t("缓存读取"), fmtTokens(u.cacheReadTokens)],
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
          {t("估算成本")} <strong>${data.totalsEstimatedCostUsd.toFixed(2)}</strong>
          {(data.subagents?.length ?? 0) > 0 && (
            <span className={styles.dimNote}>
              {t("（含 {0} 个子 agent）", data.subagents!.length)}
            </span>
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
  const nav = useAgentNav();
  const data = useRelayData<WorkflowTree[]>(client, "workflow_trees", {
    path: session.jsonlPath,
  });

  if (data === "loading") return <Hint>{t("加载 workflow…")}</Hint>;
  if (data === "error") return <Hint>{t("加载失败（桌面端可能离线）")}</Hint>;
  if (data.length === 0)
    return <EmptyState compact icon={Workflow} title={t("该会话没有 workflow 运行")} />;

  return (
    <div className={styles.stack}>
      {data.map((tree) => (
        <div key={tree.runId} className={styles.planCard}>
          <div className={styles.planTitle}>{tree.name || tree.runId}</div>
          {tree.description && <div className={styles.dimNote}>{tree.description}</div>}
          {tree.agents.map((a, i) => {
            // Drillable when the snapshot surfaced this workflow agent's
            // transcript (`agent-<id>` row present). Same nav as the Agent card.
            const canOpen = !!a.agentId && !!nav?.has(a.agentId);
            return (
              <div
                key={a.agentId ?? i}
                className={styles.wfAgent}
                data-open={canOpen || undefined}
                role={canOpen ? "button" : undefined}
                onClick={canOpen ? () => nav!.open(a.agentId!) : undefined}
              >
                <span className={styles.wfStatus} data-status={a.status}>
                  {t(AGENT_STATUS_LABEL[a.status ?? ""] ?? a.status ?? "?")}
                </span>
                <span className={styles.wfLabel}>{a.label || a.prompt || a.agentId}</span>
                {canOpen && <ChevronRight size={14} className={styles.wfChevron} />}
              </div>
            );
          })}
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

  if (data === "loading") return <Hint>{t("加载接力链…")}</Hint>;
  if (data === "error") return <Hint>{t("加载失败（桌面端可能离线）")}</Hint>;
  // The relay resolves a null reply to an empty placeholder object, so a real
  // chain is only present when it carries a chainId.
  if (!data || !data.chainId) return <EmptyState compact icon={Waypoints} title={t("该会话不在任何接力链上")} />;

  const hops: string[] = [];
  for (const l of data.links) {
    if (hops[hops.length - 1] !== l.fromSessionId) hops.push(l.fromSessionId);
    hops.push(l.toSessionId);
  }

  return (
    <div className={styles.stack}>
      <div className={styles.dimNote}>
        {t("接力 {0} 棒", hops.length)}
        {hops.includes(session.id) && ` · ${t("当前第 {0} 棒", hops.indexOf(session.id) + 1)}`}
      </div>
      {data.links.map((l, i) => (
        <HandoffLinkCard
          key={i}
          link={l}
          index={i}
          // Auto-expand the leg the viewer is on; a long chain stays scannable
          // with every other note collapsed until tapped.
          defaultOpen={l.fromSessionId === session.id || l.toSessionId === session.id}
        />
      ))}
    </div>
  );
}

/** One relay leg: a tap-to-expand card. Collapsed shows a two-line note preview;
 *  expanded renders the full markdown note. */
function HandoffLinkCard({
  link,
  index,
  defaultOpen,
}: {
  link: HandoffChain["links"][number];
  index: number;
  defaultOpen: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const preview = link.note.replace(/\s+/g, " ").trim();
  const toggle = () => setOpen((v) => !v);

  return (
    <div className={styles.planCard}>
      <div
        className={styles.hopHead}
        role="button"
        tabIndex={0}
        aria-expanded={open}
        style={{ cursor: "pointer" }}
        onClick={toggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            toggle();
          }
        }}
      >
        <ChevronRight
          size={14}
          className={styles.hopChevron}
          style={{ transform: open ? "rotate(90deg)" : "none" }}
        />
        <span className={styles.hopBadge}>{t("第 {0} → {1} 棒", index + 1, index + 2)}</span>
        <span className={styles.recordTime}>
          {new Date(link.handedAt).toLocaleString(dateLocale(), {
            month: "2-digit",
            day: "2-digit",
            hour: "2-digit",
            minute: "2-digit",
          })}
        </span>
      </div>
      {(link.planId || link.nextTask) && (
        <div className={styles.dimNote}>
          {link.planId && t("计划 {0}", link.planId)}
          {link.nextTask && ` · ${t("下一步 {0}", link.nextTask)}`}
        </div>
      )}
      {open ? (
        <div className={styles.markdown}>
          <ReactMarkdown remarkPlugins={mdRemarkPlugins} rehypePlugins={mdRehypePlugins}>{link.note}</ReactMarkdown>
        </div>
      ) : (
        <div className={styles.notePreview}>{preview}</div>
      )}
    </div>
  );
}
