import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { getLang, t } from "../i18n";
import type { RelayClient } from "../relay";
import type {
  A2uiRenderRequest,
  CommandLeaf,
  ContentBlock,
  ElicitationQuestion,
  FleetAskFormField,
  FleetAskQuestion,
  FleetAskRequest,
  GuardRequest,
  PendingDecision,
  PermissionPromptRequest,
  PlanApprovalRequest,
  RawMessage,
  SessionInfo,
} from "../types";
import type { Attachment } from "./Composer";
import { uploadAttachmentFiles } from "./Composer";
import styles from "./DecisionsView.module.css";
import { StructuredCommand } from "./StructuredCommand";

const KIND_LABEL: Record<string, string> = {
  guard: "命令审批",
  elicitation: "问题请示",
  "fleet-ask": "决策卡",
  "plan-approval": "计划审批",
  "permission-prompt": "权限请求",
  "a2ui-render": "Agent 界面",
};

interface Props {
  decisions: PendingDecision[];
  client: RelayClient | null;
  agentOnline: boolean;
  workspaceOf: (sessionId: string) => SessionInfo | undefined;
  onAnswered: (id: string) => void;
  onOpenSession: (sessionId: string) => void;
}

export function DecisionsView({
  decisions,
  client,
  agentOnline,
  workspaceOf,
  onAnswered,
  onOpenSession,
}: Props) {
  // One focused card at a time + a queue bar, mirroring the desktop panel's
  // active-card/tab model. When the active card resolves, focus falls to the
  // card at the same queue position (desktop's "next after removed").
  const [activeId, setActiveId] = useState<string | null>(null);
  const lastIndexRef = useRef(0);

  const sorted = useMemo(
    () => [...decisions].sort((a, b) => a.arrivedAt - b.arrivedAt),
    [decisions],
  );
  const foundIndex = sorted.findIndex((d) => d.id === activeId);
  const activeIndex =
    foundIndex >= 0 ? foundIndex : Math.min(lastIndexRef.current, sorted.length - 1);
  useEffect(() => {
    if (foundIndex >= 0) lastIndexRef.current = foundIndex;
  }, [foundIndex]);
  const active = sorted[activeIndex];

  if (decisions.length === 0) {
    return (
      <div className={styles.empty}>
        {agentOnline ? t("没有待处理的决策 🎉") : t("桌面端离线，暂时收不到新决策")}
      </div>
    );
  }

  return (
    <div className={styles.list}>
      {sorted.length > 1 && (
        <div className={styles.queueBar}>
          {sorted.map((d, i) => {
            const s = workspaceOf(d.request.sessionId);
            const ws = d.request.workspaceName || s?.workspaceName || "Fleet";
            return (
              <button
                key={d.id}
                className={styles.queueChip}
                data-active={i === activeIndex}
                data-kind={d.kind}
                onClick={() => setActiveId(d.id)}
              >
                <span className={styles.queueKind}>{t(KIND_LABEL[d.kind] ?? d.kind)}</span>
                <span className={styles.queueWs}>{ws}</span>
              </button>
            );
          })}
        </div>
      )}
      {active && (
        <DecisionCard
          key={active.id}
          decision={active}
          client={client}
          workspaceOf={workspaceOf}
          onAnswered={onAnswered}
          onOpenSession={onOpenSession}
        />
      )}
      {sorted.length > 1 && (
        <div className={styles.queueHint}>
          {t("第 {0} / {1} 张 · 处理完自动跳到下一张", activeIndex + 1, sorted.length)}
        </div>
      )}
    </div>
  );
}

interface CardProps {
  decision: PendingDecision;
  client: RelayClient | null;
  workspaceOf: (sessionId: string) => SessionInfo | undefined;
  onAnswered: (id: string) => void;
  onOpenSession: (sessionId: string) => void;
}

function DecisionCard({ decision, client, workspaceOf, onAnswered, onOpenSession }: CardProps) {
  const req = decision.request;
  const session = workspaceOf(req.sessionId);
  const workspace = req.workspaceName || session?.workspaceName || "Fleet";
  const aiTitle = req.aiTitle || session?.aiTitle;

  const submit = useCallback(
    (fields: Record<string, unknown>) => {
      if (!client) return;
      if (client.answer(decision.kind, decision.id, fields)) {
        onAnswered(decision.id);
      }
    },
    [client, decision.kind, decision.id, onAnswered],
  );

  return (
    <div className={styles.card}>
      <div className={styles.cardHead}>
        <span className={styles.kindChip} data-kind={decision.kind}>
          {t(KIND_LABEL[decision.kind] ?? decision.kind)}
        </span>
        <span className={styles.workspace}>{workspace}</span>
        {session && (
          <button
            className={styles.sessionLink}
            onClick={() => onOpenSession(session.id)}
            aria-label={t("查看会话详情")}
          >
            {t("会话")} ›
          </button>
        )}
      </div>
      {aiTitle && <div className={styles.aiTitle}>{aiTitle}</div>}
      {decision.kind === "guard" && (
        <GuardCard request={req as GuardRequest} client={client} session={session} submit={submit} />
      )}
      {decision.kind === "permission-prompt" && (
        <PermissionCard request={req as PermissionPromptRequest} submit={submit} />
      )}
      {decision.kind === "plan-approval" && (
        <PlanCard request={req as PlanApprovalRequest} submit={submit} />
      )}
      {decision.kind === "a2ui-render" && (
        <A2uiCard request={req as A2uiRenderRequest} submit={submit} />
      )}
      {(decision.kind === "elicitation" || decision.kind === "fleet-ask") && (
        <QuestionsCard
          request={req as FleetAskRequest}
          isFleetAsk={decision.kind === "fleet-ask"}
          client={client}
          session={session}
          submit={submit}
        />
      )}
    </div>
  );
}

// ── guard ────────────────────────────────────────────────────────────────────

function looksLikeSubcommand(tok: string): boolean {
  return /^[a-z][a-z0-9_-]*$/.test(tok);
}

/** `argv[0]` plus the first bare-word subcommand (git push / npm test),
 *  skipping flags and paths — same heuristic as the desktop panel. */
function computeLeafAllowPrefix(argv: string[]): string {
  const head = argv[0];
  if (!head) return "";
  const sub = argv.slice(1).find((t) => looksLikeSubcommand(t));
  return sub ? `${head} ${sub}` : head;
}

/** One prefix per AST leaf that fired the audit and isn't already covered;
 *  legacy payloads (no triggering flags) fall back to every leaf, and a
 *  missing AST falls back to the raw command's first line. */
function computeGuardAllowPrefixes(req: GuardRequest): string[] {
  const view = req.structuredCommand;
  if (view && view.leaves.length > 0) {
    const anyTriggering = view.leaves.some((leaf: CommandLeaf) => leaf.triggering === true);
    const eligible = anyTriggering
      ? view.leaves.filter((leaf) => leaf.triggering === true && leaf.already_allowed !== true)
      : view.leaves;
    const out: string[] = [];
    const seen = new Set<string>();
    for (const leaf of eligible) {
      const p = computeLeafAllowPrefix(leaf.argv);
      if (p && !seen.has(p)) {
        seen.add(p);
        out.push(p);
      }
    }
    if (out.length > 0) return out;
    if (anyTriggering) return []; // all triggering leaves already covered
  }
  const firstLine = req.command.split("\n")[0]?.trim() ?? "";
  const fallback = firstLine
    ? computeLeafAllowPrefix(firstLine.split(/\s+/).filter((t) => t.length > 0))
    : "";
  return fallback ? [fallback] : [];
}

/** Walk the transcript tail backwards for the last assistant prose — the same
 *  context the desktop's get_guard_context feeds the LLM analysis. */
function lastAssistantText(rows: RawMessage[]): string {
  for (let i = rows.length - 1; i >= 0; i--) {
    const msg = rows[i];
    if (msg.type !== "assistant" || msg.isSidechain) continue;
    const content = msg.message?.content;
    if (typeof content === "string") {
      if (content.trim()) return content.trim();
      continue;
    }
    if (!Array.isArray(content)) continue;
    const text = content
      .filter((b) => b.type === "text" && b.text?.trim())
      .map((b) => b.text)
      .join("\n\n");
    if (text) return text;
  }
  return "";
}

/** Auto-triggered LLM risk analysis for a guard card. */
function GuardAnalysis({
  request,
  client,
  session,
}: {
  request: GuardRequest;
  client: RelayClient | null;
  session: SessionInfo | undefined;
}) {
  const [state, setState] = useState<"loading" | "unavailable" | string>("loading");
  const fired = useRef(false);

  useEffect(() => {
    if (!client || fired.current) return;
    fired.current = true;
    let cancelled = false;
    (async () => {
      let context = "";
      if (session?.jsonlPath) {
        try {
          const rows = await client.request<RawMessage[]>("tail", {
            path: session.jsonlPath,
            n: 20,
          });
          context = lastAssistantText(rows);
        } catch {
          // context is best-effort
        }
      }
      try {
        const { analysis } = await client.request<{ analysis: string }>(
          "guard_analyze",
          // Follow the app's language setting (defaults to device locale).
          { command: request.command, context, lang: getLang() },
          40_000, // the desktop-side LLM call itself may take up to 30s
        );
        if (!cancelled) setState(analysis);
      } catch {
        if (!cancelled) setState("unavailable");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, request.command, request.id, session?.jsonlPath]);

  if (state === "unavailable") return null;
  return (
    <div className={styles.analysis}>
      <div className={styles.analysisHead}>{t("AI 风险分析")}</div>
      {state === "loading" ? (
        <div className={styles.analysisLoading}>{t("分析中…")}</div>
      ) : (
        <div className={styles.markdown}>
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{state}</ReactMarkdown>
        </div>
      )}
    </div>
  );
}

function GuardCard({
  request,
  client,
  session,
  submit,
}: {
  request: GuardRequest;
  client: RelayClient | null;
  session: SessionInfo | undefined;
  submit: (f: Record<string, unknown>) => void;
}) {
  const [reason, setReason] = useState("");
  const [showReason, setShowReason] = useState(false);
  const [prefixMenuOpen, setPrefixMenuOpen] = useState(false);
  const allowPrefixes = useMemo(() => computeGuardAllowPrefixes(request), [request]);
  const sourceTag = request.riskTags[0] ?? null;

  const alwaysAllow = (prefix: string) =>
    submit({ allow: true, alwaysAllow: { prefix, sourceTag } });

  return (
    <div>
      <StructuredCommand command={request.command} view={request.structuredCommand} />
      {request.riskTags.length > 0 && (
        <div className={styles.chipRow}>
          {request.riskTags.map((t) => (
            <span key={t} className={styles.riskChip}>
              {t}
            </span>
          ))}
        </div>
      )}
      <GuardAnalysis request={request} client={client} session={session} />
      {showReason && (
        <textarea
          className={styles.reasonInput}
          placeholder={t("拒绝理由（可选，会转告给 AI）")}
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          rows={2}
        />
      )}
      {prefixMenuOpen && allowPrefixes.length > 1 && (
        <div className={styles.prefixMenu}>
          {allowPrefixes.map((p) => (
            <button key={p} className={styles.prefixItem} onClick={() => alwaysAllow(p)}>
              {t("总是允许")} <code>{p}</code>
            </button>
          ))}
        </div>
      )}
      <div className={styles.actions}>
        <button
          className={styles.dangerButton}
          onClick={() => {
            if (!showReason) {
              setShowReason(true);
              return;
            }
            submit({ allow: false, reason: reason || undefined });
          }}
        >
          {showReason ? t("确认拒绝") : t("拒绝")}
        </button>
        {allowPrefixes.length > 0 && (
          <button
            className={styles.ghostButton}
            onClick={() => {
              if (allowPrefixes.length === 1) alwaysAllow(allowPrefixes[0]);
              else setPrefixMenuOpen((v) => !v);
            }}
          >
            {allowPrefixes.length === 1 ? t("总是允许 {0}", allowPrefixes[0]) : t("总是允许…")}
          </button>
        )}
        <button className={styles.primaryButton} onClick={() => submit({ allow: true })}>
          {t("允许")}
        </button>
      </div>
    </div>
  );
}

// ── permission-prompt ────────────────────────────────────────────────────────

function PermissionCard({
  request,
  submit,
}: {
  request: PermissionPromptRequest;
  submit: (f: Record<string, unknown>) => void;
}) {
  const [reason, setReason] = useState("");
  const [showReason, setShowReason] = useState(false);
  const input = useMemo(() => {
    try {
      return JSON.stringify(request.toolInput, null, 2);
    } catch {
      return String(request.toolInput);
    }
  }, [request.toolInput]);
  return (
    <div>
      <div className={styles.toolName}>
        {t("工具：")}
        <code>{request.toolName}</code>
      </div>
      {input && input !== "null" && <pre className={styles.command}>{truncate(input, 6000)}</pre>}
      {showReason && (
        <textarea
          className={styles.reasonInput}
          placeholder={t("拒绝理由（可选）")}
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          rows={2}
        />
      )}
      <div className={styles.actions}>
        <button
          className={styles.dangerButton}
          onClick={() => {
            if (!showReason) {
              setShowReason(true);
              return;
            }
            submit({ allow: false, reason: reason || undefined });
          }}
        >
          {showReason ? t("确认拒绝") : t("拒绝")}
        </button>
        <button className={styles.primaryButton} onClick={() => submit({ allow: true })}>
          {t("允许")}
        </button>
      </div>
    </div>
  );
}

// ── plan-approval ────────────────────────────────────────────────────────────

function PlanCard({
  request,
  submit,
}: {
  request: PlanApprovalRequest;
  submit: (f: Record<string, unknown>) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [rejecting, setRejecting] = useState(false);
  const [feedback, setFeedback] = useState("");
  const [editing, setEditing] = useState(false);
  const [editedPlan, setEditedPlan] = useState(request.planContent);
  const long = request.planContent.length > 600;
  const content = expanded || !long ? request.planContent : request.planContent.slice(0, 600);
  const edited = editing && editedPlan.trim() !== request.planContent.trim();
  return (
    <div>
      {request.planFilePath && <div className={styles.planPath}>{request.planFilePath}</div>}
      {editing ? (
        <textarea
          className={styles.planEditor}
          value={editedPlan}
          onChange={(e) => setEditedPlan(e.target.value)}
          rows={12}
        />
      ) : (
        <div className={styles.markdown}>
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
          {long && (
            <button className={styles.expandButton} onClick={() => setExpanded((v) => !v)}>
              {expanded ? t("收起") : t("展开完整计划")}
            </button>
          )}
        </div>
      )}
      <button
        className={styles.editToggle}
        onClick={() =>
          setEditing((v) => {
            // Leaving edit mode discards the draft (desktop "Cancel edit"
            // semantics) — otherwise a stale draft looks kept but never sends.
            if (v) setEditedPlan(request.planContent);
            return !v;
          })
        }
      >
        {editing ? t("放弃编辑") : t("✎ 编辑计划")}
      </button>
      {rejecting && (
        <textarea
          className={styles.reasonInput}
          placeholder={t("驳回意见（会转告给 AI 修改计划）")}
          value={feedback}
          onChange={(e) => setFeedback(e.target.value)}
          rows={3}
        />
      )}
      <div className={styles.actions}>
        <button
          className={styles.dangerButton}
          onClick={() => {
            if (!rejecting) {
              setRejecting(true);
              return;
            }
            submit({ decision: "reject", feedback: feedback || undefined });
          }}
        >
          {rejecting ? t("确认驳回") : t("驳回")}
        </button>
        <button
          className={styles.primaryButton}
          onClick={() =>
            submit(
              edited
                ? { decision: "approve", editedPlan }
                : { decision: "approve" },
            )
          }
        >
          {edited ? t("批准已编辑版") : t("批准")}
        </button>
      </div>
    </div>
  );
}

// ── a2ui-render ──────────────────────────────────────────────────────────────

/** Best-effort text scrape of an A2UI message tree, so the placeholder card
 *  can hint at what the surface asks instead of showing an opaque blob. */
function extractA2uiTexts(tree: unknown, out: string[] = []): string[] {
  if (out.length >= 8 || tree === null || typeof tree !== "object") return out;
  if (Array.isArray(tree)) {
    for (const item of tree) extractA2uiTexts(item, out);
    return out;
  }
  for (const [key, value] of Object.entries(tree as Record<string, unknown>)) {
    if (out.length >= 8) break;
    if (typeof value === "string") {
      if ((key === "text" || key === "label" || key === "title") && value.trim()) {
        out.push(value.trim());
      }
    } else {
      extractA2uiTexts(value, out);
    }
  }
  return out;
}

/** fleet__render_a2ui placeholder: the mobile client cannot render the A2UI
 *  surface (no @a2ui runtime), but it must never swallow the decision — show
 *  what we can and let Boss cancel so the agent isn't left waiting blind. */
function A2uiCard({
  request,
  submit,
}: {
  request: A2uiRenderRequest;
  submit: (f: Record<string, unknown>) => void;
}) {
  const [cancelling, setCancelling] = useState(false);
  const texts = useMemo(() => extractA2uiTexts(request.messageTree), [request.messageTree]);
  return (
    <div>
      <div className={styles.a2uiNotice}>
        {t("Agent 发来一张自定义界面（A2UI）。移动端暂不支持渲染，请在桌面端处理这张卡。")}
      </div>
      {texts.length > 0 && (
        <ul className={styles.a2uiTexts}>
          {texts.map((t, i) => (
            <li key={i}>{truncate(t, 120)}</li>
          ))}
        </ul>
      )}
      <div className={styles.actions}>
        <button
          className={styles.dangerButton}
          onClick={() => {
            if (!cancelling) {
              setCancelling(true);
              return;
            }
            submit({ cancelled: true, actionContext: {} });
          }}
        >
          {cancelling ? t("确认取消这张卡") : t("取消（告知 agent）")}
        </button>
      </div>
    </div>
  );
}

// ── preceding agent narration ────────────────────────────────────────────────
// Port of the desktop's usePrecedingAgentMessages slicing: the agent's plain
// prose since the user's last real input, shown collapsed above the question.

const NARRATION_TAIL = 200;
const NARRATION_MAX_CHUNKS = 40;

function isAskToolName(name: string): boolean {
  return (
    name === "AskUserQuestion" ||
    name === "ExitPlanMode" ||
    name === "mcp__fleet__fleet__render_a2ui" ||
    name.includes("fleet__ask")
  );
}

function blocksOf(msg: RawMessage): ContentBlock[] {
  const content = msg.message?.content;
  return Array.isArray(content) ? content : [];
}

function assistantProse(msg: RawMessage): string {
  if (msg.type !== "assistant" || !msg.message) return "";
  const content = msg.message.content;
  if (typeof content === "string") return content.trim();
  return blocksOf(msg)
    .filter((b) => b.type === "text" && b.text)
    .map((b) => b.text)
    .join("\n")
    .trim();
}

/** Agent narration since the user's last input (typed prompt or an answer to
 *  an ask-family tool; plain Bash/Read tool_results do NOT end the span). */
function slicePrecedingMessages(messages: RawMessage[]): { key: string; text: string }[] {
  const toolNameById = new Map<string, string>();
  for (const m of messages) {
    for (const b of blocksOf(m)) {
      if (b.type === "tool_use" && b.id && b.name) toolNameById.set(b.id, b.name);
    }
  }
  const isUserInput = (msg: RawMessage): boolean => {
    if (msg.type !== "user" || !msg.message) return false;
    const content = msg.message.content;
    if (typeof content === "string") return content.trim().length > 0;
    const blocks = blocksOf(msg);
    if (blocks.some((b) => b.type === "text" && b.text?.trim())) return true;
    return blocks.some((b) => {
      if (b.type !== "tool_result" || !b.tool_use_id) return false;
      const name = toolNameById.get(b.tool_use_id);
      return !!name && isAskToolName(name);
    });
  };
  let boundary = -1;
  for (let i = messages.length - 1; i >= 0; i--) {
    if (isUserInput(messages[i])) {
      boundary = i;
      break;
    }
  }
  const out: { key: string; text: string }[] = [];
  for (const [i, m] of messages.slice(boundary + 1).entries()) {
    const text = assistantProse(m);
    if (text) out.push({ key: m.uuid ?? String(i), text });
  }
  return out.length > NARRATION_MAX_CHUNKS ? out.slice(out.length - NARRATION_MAX_CHUNKS) : out;
}

function PrecedingNarration({
  session,
  client,
}: {
  session: SessionInfo | undefined;
  client: RelayClient | null;
}) {
  const [chunks, setChunks] = useState<{ key: string; text: string }[]>([]);
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    if (!client || !session?.jsonlPath) return;
    let cancelled = false;
    client
      .request<RawMessage[]>("tail", { path: session.jsonlPath, n: NARRATION_TAIL })
      .then((rows) => {
        if (!cancelled) setChunks(slicePrecedingMessages(rows));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [client, session?.jsonlPath]);

  if (chunks.length === 0) return null;
  return (
    <div className={styles.preceding}>
      <button className={styles.precedingToggle} onClick={() => setExpanded((v) => !v)}>
        {expanded ? t("▾ 收起提问前的说明") : t("▸ Agent 干活时还说了 {0} 段话", chunks.length)}
      </button>
      {expanded && (
        <div className={styles.precedingBody}>
          {chunks.map((c) => (
            <div key={c.key} className={styles.markdown}>
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{c.text}</ReactMarkdown>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── elicitation / fleet-ask ──────────────────────────────────────────────────

const OTHER = "__other__";

function QuestionsCard({
  request,
  isFleetAsk,
  client,
  session,
  submit,
}: {
  request: FleetAskRequest;
  isFleetAsk: boolean;
  client: RelayClient | null;
  session: SessionInfo | undefined;
  submit: (f: Record<string, unknown>) => void;
}) {
  // question text → selected labels (OTHER = custom text active)
  const [selections, setSelections] = useState<Record<string, string[]>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});
  // question text → user flipped a single-select into multi-select
  const [multiOverride, setMultiOverride] = useState<Record<string, boolean>>({});
  // question text → uploaded attachments appended to the answer as @path
  const [attachments, setAttachments] = useState<Record<string, Attachment[]>>({});
  const [uploadingQ, setUploadingQ] = useState<string | null>(null);
  const [form, setForm] = useState<Record<string, string>>(() => {
    const init: Record<string, string> = {};
    for (const q of request.questions) {
      for (const f of q.formFields ?? []) {
        if (f.default !== undefined && f.default !== null) init[f.name] = String(f.default);
        else if (f.kind === "checkbox") init[f.name] = "false";
        else if (f.kind === "range") init[f.name] = String(f.min ?? 0);
      }
    }
    return init;
  });
  const [error, setError] = useState<string | null>(null);
  // Stepped wizard over 2+ questions, mirroring the desktop panel.
  const [step, setStep] = useState(0);
  // `${question} ${label}` → preview expanded before selection.
  const [previewOpen, setPreviewOpen] = useState<Record<string, boolean>>({});

  /** Desktop `hasAnswer`: option / custom text / attachment / form-only,
   *  with required form fields filled. Gates Next/Submit instead of erroring
   *  after the fact. */
  const answeredQuestion = (q: ElicitationQuestion | FleetAskQuestion): boolean => {
    const hasOptions = (q.options?.length ?? 0) > 0;
    const sel = selections[q.question] ?? [];
    const picked = sel.filter((l) => l !== OTHER);
    const customActive = !hasOptions || sel.includes(OTHER);
    const customText = customActive ? (custom[q.question] ?? "").trim() : "";
    const atts = attachments[q.question] ?? [];
    const fields = (q as FleetAskQuestion).formFields ?? [];
    const requiredOk = fields.every((f) => !f.required || (form[f.name] ?? "").trim());
    const answered =
      picked.length > 0 || customText.length > 0 || atts.length > 0 || fields.length > 0;
    return answered && requiredOk;
  };

  /** Desktop's per-option pencil: seed the Other composer with the option's
   *  text so it can be tweaked instead of retyped. */
  const editToOther = (q: ElicitationQuestion | FleetAskQuestion, o: { label: string; description?: string }) => {
    const effectiveMulti = q.multiSelect || multiOverride[q.question] === true;
    setSelections((prev) => {
      const cur = prev[q.question] ?? [];
      if (effectiveMulti) {
        return cur.includes(OTHER) ? prev : { ...prev, [q.question]: [...cur, OTHER] };
      }
      return { ...prev, [q.question]: [OTHER] };
    });
    setCustom((p) => ({
      ...p,
      [q.question]: o.description ? `${o.label} — ${o.description}` : o.label,
    }));
  };

  const toggle = (q: ElicitationQuestion | FleetAskQuestion, label: string) => {
    const effectiveMulti = q.multiSelect || multiOverride[q.question] === true;
    setSelections((prev) => {
      const cur = prev[q.question] ?? [];
      if (effectiveMulti) {
        return {
          ...prev,
          [q.question]: cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label],
        };
      }
      return { ...prev, [q.question]: cur.includes(label) ? [] : [label] };
    });
  };

  const flipMultiOverride = (q: ElicitationQuestion | FleetAskQuestion) => {
    setMultiOverride((prev) => {
      const next = !prev[q.question];
      if (!next) {
        // Back to single-select: trim selections to at most one.
        setSelections((sel) => {
          const cur = sel[q.question] ?? [];
          return cur.length > 1 ? { ...sel, [q.question]: [cur[0]] } : sel;
        });
      }
      return { ...prev, [q.question]: next };
    });
  };

  const addQuestionFiles = async (question: string, files: FileList | null) => {
    if (!client || !files || files.length === 0) return;
    setUploadingQ(question);
    try {
      const uploaded = await uploadAttachmentFiles(client, files);
      setAttachments((prev) => {
        const cur = prev[question] ?? [];
        const next = [...cur];
        for (const a of uploaded) {
          if (!next.some((x) => x.path === a.path)) next.push(a);
        }
        return { ...prev, [question]: next };
      });
    } catch (e) {
      window.alert(e instanceof Error ? e.message : t("附件上传失败"));
    } finally {
      setUploadingQ(null);
    }
  };

  const doSubmit = (declined: boolean) => {
    if (declined) {
      submit(isFleetAsk ? { cancelled: true, answers: {} } : { declined: true, answers: {} });
      return;
    }
    const answers: Record<string, string> = {};
    for (const q of request.questions) {
      const hasOptions = (q.options?.length ?? 0) > 0;
      const picked = (selections[q.question] ?? []).filter((l) => l !== OTHER);
      // Option-less questions expose the free-text box directly (no "其他…"
      // toggle), mirroring the desktop's always-present composer.
      const customActive = !hasOptions || (selections[q.question] ?? []).includes(OTHER);
      const customText = customActive ? (custom[q.question] ?? "").trim() : "";
      const parts = [...picked];
      if (customText) parts.push(customText);
      const atts0 = attachments[q.question] ?? [];
      const hasForm = (q.formFields?.length ?? 0) > 0;
      // Desktop gates submit on option / custom text / attachment (form-only
      // questions are covered by the required-field pass below).
      if (parts.length === 0 && atts0.length === 0 && !hasForm) {
        setError(t("「{0}」还没有作答", q.header || truncate(q.question, 20)));
        return;
      }
      let answer = parts.join(", ");
      // Same marker string the desktop appends when the user flipped a
      // single-select question into multi-select.
      if (answer && multiOverride[q.question] === true && !q.multiSelect) {
        answer = `${answer} [用户将此题从单选改为多选 / user switched this question from single-select to multi-select]`;
      }
      const atts = attachments[q.question] ?? [];
      if (atts.length > 0) {
        const mentions = atts.map((a) => `@${a.path}`).join(" ");
        answer = answer ? `${answer} ${mentions}` : mentions;
      }
      if (answer) answers[q.question] = answer;
    }
    for (const q of request.questions) {
      for (const f of q.formFields ?? []) {
        const v = form[f.name] ?? "";
        if (f.required && !v.trim()) {
          setError(t("「{0}」是必填项", f.label));
          return;
        }
        answers[f.name] = v;
      }
    }
    submit(isFleetAsk ? { cancelled: false, answers } : { declined: false, answers });
  };

  const total = request.questions.length;
  const multiQ = total > 1;
  const qi = Math.min(step, total - 1);
  const allAnswered = request.questions.every(answeredQuestion);

  return (
    <div>
      <PrecedingNarration session={session} client={client} />
      {multiQ && (
        <div className={styles.stepBar}>
          <span className={styles.stepLabel}>
            {t("第 {0} / {1} 题", qi + 1, total)}
          </span>
          <div className={styles.stepDots}>
            {request.questions.map((qq, i) => (
              <button
                key={i}
                className={styles.stepDot}
                data-state={i === qi ? "active" : answeredQuestion(qq) ? "done" : "todo"}
                onClick={() => setStep(i)}
                aria-label={t("第 {0} 题", i + 1)}
              />
            ))}
          </div>
        </div>
      )}
      {[request.questions[qi]].map((q) => (
        <div key={qi} className={styles.question}>
          {q.header && <div className={styles.questionHeader}>{q.header}</div>}
          <div className={styles.markdown}>
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{stripTtsDivider(q.question)}</ReactMarkdown>
          </div>
          {isFleetAsk && (q as FleetAskQuestion).html && (
            <HtmlPreview
              html={(q as FleetAskQuestion).html!}
              images={(q as FleetAskQuestion).images}
              requestId={request.id}
              qidx={qi}
              client={client}
            />
          )}
          {isFleetAsk && !(q as FleetAskQuestion).html && (q as FleetAskQuestion).images?.length ? (
            <ImageGallery
              images={(q as FleetAskQuestion).images!}
              requestId={request.id}
              qidx={qi}
              client={client}
            />
          ) : null}
          {(q.options ?? []).map((o) => {
            const selected = (selections[q.question] ?? []).includes(o.label);
            const effectiveMulti = q.multiSelect || multiOverride[q.question] === true;
            const previewKey = `${q.question} ${o.label}`;
            // Desktop shows a focused option's preview BEFORE selection (the
            // point of preview is comparing candidates); no hover on touch,
            // so an explicit toggle stands in.
            const previewShown = o.preview ? selected || previewOpen[previewKey] === true : false;
            return (
              <div key={o.label}>
                <div className={styles.optionRow}>
                  <button
                    className={styles.option}
                    data-selected={selected}
                    onClick={() => toggle(q, o.label)}
                  >
                    <span className={styles.optionMark} data-multi={effectiveMulti}>
                      {selected ? "✓" : ""}
                    </span>
                    <span className={styles.optionBody}>
                      <span className={styles.optionLabel}>{o.label}</span>
                      {o.description && <span className={styles.optionDesc}>{o.description}</span>}
                    </span>
                  </button>
                  <div className={styles.optionSide}>
                    {o.preview && (
                      <button
                        className={styles.optionSideBtn}
                        data-active={previewShown}
                        onClick={() =>
                          setPreviewOpen((p) => ({ ...p, [previewKey]: !previewShown }))
                        }
                        aria-label={t("预览此选项")}
                      >
                        👁
                      </button>
                    )}
                    <button
                      className={styles.optionSideBtn}
                      onClick={() => editToOther(q, o)}
                      aria-label={t("编辑此选项（复制到「其他」）")}
                    >
                      ✎
                    </button>
                  </div>
                </div>
                {previewShown && o.preview && (
                  <div className={styles.optionPreview}>
                    <ReactMarkdown remarkPlugins={[remarkGfm]}>{o.preview}</ReactMarkdown>
                  </div>
                )}
              </div>
            );
          })}
          {(q.options?.length ?? 0) > 0 ? (
            <>
              <button
                className={styles.option}
                data-selected={(selections[q.question] ?? []).includes(OTHER)}
                onClick={() => toggle(q, OTHER)}
              >
                <span
                  className={styles.optionMark}
                  data-multi={q.multiSelect || multiOverride[q.question] === true}
                >
                  {(selections[q.question] ?? []).includes(OTHER) ? "✓" : ""}
                </span>
                <span className={styles.optionBody}>
                  <span className={styles.optionLabel}>{t("其他…")}</span>
                </span>
              </button>
              {(selections[q.question] ?? []).includes(OTHER) && (
                <textarea
                  className={styles.reasonInput}
                  placeholder={t("自定义回答")}
                  value={custom[q.question] ?? ""}
                  onChange={(e) => setCustom((p) => ({ ...p, [q.question]: e.target.value }))}
                  rows={2}
                />
              )}
            </>
          ) : (
            // No options: the free-text box is the answer surface itself,
            // like the desktop's always-rendered composer (form-only cards
            // may leave it empty; required fields gate submit instead).
            <textarea
              className={styles.reasonInput}
              placeholder={t("自由回答…")}
              value={custom[q.question] ?? ""}
              onChange={(e) => setCustom((p) => ({ ...p, [q.question]: e.target.value }))}
              rows={2}
            />
          )}
          <div className={styles.questionTools}>
            {(q.options?.length ?? 0) > 0 && !q.multiSelect && (
              <button
                className={styles.toolToggle}
                data-active={multiOverride[q.question] === true}
                onClick={() => flipMultiOverride(q)}
              >
                {multiOverride[q.question] ? t("已改为多选") : t("改为多选")}
              </button>
            )}
            <QuestionAttachRow
              question={q.question}
              attachments={attachments[q.question] ?? []}
              uploading={uploadingQ === q.question}
              onPick={(files) => void addQuestionFiles(q.question, files)}
              onRemove={(path) =>
                setAttachments((prev) => ({
                  ...prev,
                  [q.question]: (prev[q.question] ?? []).filter((a) => a.path !== path),
                }))
              }
            />
          </div>
          {(q.formFields ?? []).map((f) => (
            <FormFieldControl
              key={f.name}
              field={f}
              value={form[f.name] ?? ""}
              onChange={(v) => setForm((p) => ({ ...p, [f.name]: v }))}
            />
          ))}
        </div>
      ))}
      {error && <div className={styles.error}>{error}</div>}
      <div className={styles.actions}>
        <button className={styles.ghostButton} onClick={() => doSubmit(true)}>
          {isFleetAsk ? t("取消") : t("拒绝回答")}
        </button>
        {multiQ && qi > 0 && (
          <button className={styles.ghostButton} onClick={() => setStep(qi - 1)}>
            {t("上一题")}
          </button>
        )}
        {multiQ && qi < total - 1 ? (
          <button
            className={styles.primaryButton}
            disabled={!answeredQuestion(request.questions[qi])}
            onClick={() => setStep(qi + 1)}
          >
            {t("下一题")}
          </button>
        ) : (
          <button
            className={styles.primaryButton}
            disabled={!allAnswered}
            onClick={() => doSubmit(false)}
          >
            {t("提交")}
          </button>
        )}
      </div>
    </div>
  );
}

/** Per-question attachment chips + picker (answers gain `@path` mentions). */
function QuestionAttachRow({
  question,
  attachments,
  uploading,
  onPick,
  onRemove,
}: {
  question: string;
  attachments: Attachment[];
  uploading: boolean;
  onPick: (files: FileList | null) => void;
  onRemove: (path: string) => void;
}) {
  const inputRef = useRef<HTMLInputElement | null>(null);
  return (
    <div className={styles.attachRow}>
      {attachments.map((a) => (
        <span key={a.path} className={styles.attachChip}>
          {a.name}
          <button className={styles.attachRemove} onClick={() => onRemove(a.path)}>
            ×
          </button>
        </span>
      ))}
      <button
        className={styles.toolToggle}
        disabled={uploading}
        onClick={() => inputRef.current?.click()}
      >
        {uploading ? t("上传中…") : t("＋ 附件")}
      </button>
      <input
        ref={inputRef}
        type="file"
        multiple
        hidden
        aria-label={t("为「{0}」添加附件", question)}
        onChange={(e) => {
          onPick(e.target.files);
          e.target.value = "";
        }}
      />
    </div>
  );
}

function FormFieldControl({
  field,
  value,
  onChange,
}: {
  field: FleetAskFormField;
  value: string;
  onChange: (v: string) => void;
}) {
  const label = (
    <div className={styles.fieldLabel}>
      {field.label}
      {field.required && <span className={styles.required}>*</span>}
    </div>
  );
  switch (field.kind) {
    case "textarea":
      return (
        <div className={styles.field}>
          {label}
          <textarea
            rows={3}
            placeholder={field.placeholder}
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    case "select":
      return (
        <div className={styles.field}>
          {label}
          <select value={value} onChange={(e) => onChange(e.target.value)}>
            <option value="">{t("请选择…")}</option>
            {(field.options ?? []).map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        </div>
      );
    case "radio":
      return (
        <div className={styles.field}>
          {label}
          <div className={styles.radioRow}>
            {(field.options ?? []).map((o) => (
              <button
                key={o}
                className={styles.radioChip}
                data-selected={value === o}
                onClick={() => onChange(o)}
              >
                {o}
              </button>
            ))}
          </div>
        </div>
      );
    case "checkbox":
      return (
        <div className={styles.field}>
          <button
            className={styles.checkboxRow}
            onClick={() => onChange(value === "true" ? "false" : "true")}
          >
            <span className={styles.optionMark} data-multi={true}>
              {value === "true" ? "✓" : ""}
            </span>
            {field.label}
          </button>
        </div>
      );
    case "range":
      return (
        <div className={styles.field}>
          {label}
          <div className={styles.rangeRow}>
            <input
              type="range"
              min={field.min ?? 0}
              max={field.max ?? 100}
              step={field.step ?? 1}
              value={value || String(field.min ?? 0)}
              onChange={(e) => onChange(e.target.value)}
            />
            <span className={styles.rangeValue}>{value || String(field.min ?? 0)}</span>
          </div>
        </div>
      );
    default: {
      const type =
        field.kind === "datetime" ? "datetime-local" : field.kind === "number" ? "number" : field.kind;
      return (
        <div className={styles.field}>
          {label}
          <input
            type={type}
            placeholder={field.placeholder}
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    }
  }
}

// ── fleet-ask html / images ──────────────────────────────────────────────────

interface AssetReply {
  mime: string;
  base64: string;
}

function useAssets(
  names: string[],
  requestId: string,
  qidx: number,
  client: RelayClient | null,
): Record<string, string> {
  const [uris, setUris] = useState<Record<string, string>>({});
  useEffect(() => {
    if (!client) return;
    let cancelled = false;
    for (const name of names) {
      client
        .request<AssetReply>("decision_asset", { id: requestId, qidx: `q${qidx}`, rel: name })
        .then((a) => {
          if (!cancelled) {
            setUris((p) => ({ ...p, [name]: `data:${a.mime};base64,${a.base64}` }));
          }
        })
        .catch(() => {});
    }
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestId, qidx, client, names.join("|")]);
  return uris;
}

// Height handshake constants — mirror the desktop's decisionFrame.ts. The
// payload crosses a trust boundary (agent-authored document), so validate and
// clamp before applying.
const FRAME_MIN_HEIGHT = 120;
const FRAME_MAX_HEIGHT = 1400;
const FRAME_DEAD_BAND = 2;

function parseFrameHeight(data: unknown): number | null {
  if (!data || typeof data !== "object") return null;
  const raw = (data as { __fleetAskHeight?: unknown }).__fleetAskHeight;
  if (typeof raw !== "number" || !Number.isFinite(raw) || raw <= 0) return null;
  return Math.min(FRAME_MAX_HEIGHT, Math.max(FRAME_MIN_HEIGHT, Math.ceil(raw)));
}

function HtmlPreview({
  html,
  images,
  requestId,
  qidx,
  client,
}: {
  html: string;
  images: { name: string }[] | undefined;
  requestId: string;
  qidx: number;
  client: RelayClient | null;
}) {
  const names = useMemo(() => (images ?? []).map((i) => i.name), [images]);
  const uris = useAssets(names, requestId, qidx, client);
  const resolved = useMemo(() => {
    let out = html;
    for (const [name, uri] of Object.entries(uris)) {
      out = out.split(`src="${name}"`).join(`src="${uri}"`);
      out = out.split(`src='${name}'`).join(`src='${uri}'`);
    }
    return out;
  }, [html, uris]);

  // `sandbox="allow-scripts"` without `allow-same-origin` (same as the desktop
  // AutoHeightFrame): the document keeps an opaque origin — no DOM/storage
  // access — but can run the height-reporting script that `mcp_ipc` injected
  // into `q.html` at request time and post `__fleetAskHeight` up.
  const ref = useRef<HTMLIFrameElement | null>(null);
  const [height, setHeight] = useState<number | null>(null);
  useEffect(() => {
    setHeight(null);
    const onMessage = (e: MessageEvent) => {
      // Opaque origins all stringify to "null" — identify our frame by source.
      if (!ref.current || e.source !== ref.current.contentWindow) return;
      const h = parseFrameHeight(e.data);
      if (h === null) return;
      setHeight((cur) => (cur === null || Math.abs(h - cur) > FRAME_DEAD_BAND ? h : cur));
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [resolved]);

  return (
    <iframe
      ref={ref}
      className={styles.htmlPreview}
      sandbox="allow-scripts"
      srcDoc={resolved}
      // CSS keeps the legacy 220px fallback for documents that never report
      // (html stored before the script injection existed).
      style={height === null ? undefined : { height: `${height}px` }}
      title={t("预览")}
    />
  );
}

function ImageGallery({
  images,
  requestId,
  qidx,
  client,
}: {
  images: { name: string; caption?: string }[];
  requestId: string;
  qidx: number;
  client: RelayClient | null;
}) {
  const names = useMemo(() => images.map((i) => i.name), [images]);
  const uris = useAssets(names, requestId, qidx, client);
  return (
    <div className={styles.gallery}>
      {images.map((img) => (
        <figure key={img.name}>
          {uris[img.name] ? (
            <img src={uris[img.name]} alt={img.caption ?? img.name} />
          ) : (
            <div className={styles.imgLoading}>{t("加载图片…")}</div>
          )}
          {img.caption && <figcaption>{img.caption}</figcaption>}
        </figure>
      ))}
    </div>
  );
}

// ── helpers ──────────────────────────────────────────────────────────────────

/** Drop the Fleet TTS divider (a lone `---` line splits summary/body). */
function stripTtsDivider(text: string): string {
  const idx = text.split("\n").findIndex((l) => l.trim() === "---");
  if (idx === -1) return text;
  const lines = text.split("\n");
  return [...lines.slice(0, idx), ...lines.slice(idx + 1)].join("\n");
}

function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n) + "…" : s;
}
