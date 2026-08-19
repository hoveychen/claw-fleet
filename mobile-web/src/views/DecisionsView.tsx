import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CloudOff,
  Eye,
  Pencil,
  Plus,
} from "lucide-react";
import { EmptyState } from "./EmptyState";
import ReactMarkdown from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { mermaidMarkdownComponents } from "../markdown/mermaidComponents";
import { fetchDecisionAsset } from "../decisionAsset";
import { IMG_ZOOM_INJECT, parseImgZoom } from "../iframeImgZoom";
import { useLightbox } from "./Lightbox";
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
import { AttachmentThumbs } from "./AttachmentThumb";
import styles from "./DecisionsView.module.css";
import { StructuredCommand } from "./StructuredCommand";
import { basename } from "./taskNotification";
import { permissionPrimary } from "./permissionPrimary";

export const KIND_LABEL: Record<string, string> = {
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
  connected: boolean;
  agentOnline: boolean;
  decisionsLoaded: boolean;
  workspaceOf: (sessionId: string) => SessionInfo | undefined;
  onAnswered: (id: string) => void;
  onOpenSession: (sessionId: string) => void;
  /** 通知点击要聚焦的卡。`nonce` 使连点同一张卡也能重新触发。 */
  focusDecision?: { id: string; nonce: number } | null;
}

export function DecisionsView({
  decisions,
  client,
  connected,
  agentOnline,
  decisionsLoaded,
  workspaceOf,
  onAnswered,
  onOpenSession,
  focusDecision,
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

  // A long card leaves the page scrolled to its action row; once it resolves,
  // the next card mounts under that same offset with its head off-screen. Snap
  // back to the top whenever the focused card changes (answered, or picked from
  // the queue bar) so each card starts from its head.
  // 通知点击指定的卡。只在它确实存在于当前队列里时才切 —— 卡可能已被别处答掉
  // (桌面端答了、或超时消失),那时保持现状比跳到一张空卡好。
  //
  // 依赖里放 nonce 而不是 focusDecision 本身:后者是每次渲染新建的对象,会让
  // effect 每帧重跑;而只依赖 id 又会让「连点同一条通知」第二次失效。
  const focusId = focusDecision?.id;
  const focusNonce = focusDecision?.nonce;
  useEffect(() => {
    if (!focusId) return;
    if (!decisions.some((d) => d.id === focusId)) return;
    setActiveId(focusId);
  }, [focusId, focusNonce, decisions]);

  const activeCardId = active?.id ?? null;
  useEffect(() => {
    if (activeCardId === null || window.scrollY === 0) return;
    window.scrollTo({ top: 0, behavior: "smooth" });
  }, [activeCardId]);

  if (decisions.length === 0) {
    // Order mirrors the task page: connectivity first, then whether the first
    // snapshot has landed, then a genuine empty. The two transient states show
    // shimmer skeleton cards so a pending card never flashes an empty state
    // first; only "desktop offline" and "nothing pending" are terminal.
    if (!connected || (agentOnline && !decisionsLoaded)) {
      return (
        <div className={styles.list} aria-busy="true" aria-label={t("正在加载决策…")}>
          <SkeletonCard />
          <SkeletonCard />
        </div>
      );
    }
    return agentOnline ? (
      <EmptyState
        icon={CheckCircle2}
        title={t("没有待处理的决策")}
        description={t("所有决策卡都已作答，收工。有新决策时会自动出现在这里。")}
      />
    ) : (
      <EmptyState
        icon={CloudOff}
        title={t("桌面端离线")}
        description={t("暂时收不到新决策，等桌面端重新上线就会同步过来。")}
      />
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

/** Shimmer placeholder in the decision-card silhouette (chip · workspace ·
 *  title · two body lines · action row), shown while the first pending snapshot
 *  is still in flight so the tab doesn't flash an empty state then pop a card. */
function SkeletonCard() {
  return (
    <div className={styles.skelCard} aria-hidden="true">
      <div className={styles.skelRow}>
        <span className={`${styles.skeleton} ${styles.skelChip}`} />
        <span className={`${styles.skeleton} ${styles.skelWs}`} />
      </div>
      <span className={`${styles.skeleton} ${styles.skelTitle}`} />
      <span className={`${styles.skeleton} ${styles.skelLine}`} />
      <span className={`${styles.skeleton} ${styles.skelLineShort}`} />
      <div className={styles.skelActions}>
        <span className={`${styles.skeleton} ${styles.skelBtn}`} />
        <span className={`${styles.skeleton} ${styles.skelBtn}`} />
      </div>
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
  // req.aiTitle already prefers the override (resolved server-side); the
  // session fallback must too, else a Codex card with no resolved req title
  // drops back to the raw first-prompt ai_title.
  const aiTitle = req.aiTitle || session?.titleOverride || session?.aiTitle;

  // Answering over the robust req/reply path: the card is removed only once the
  // answer can no longer be lost (answerViaReq resolves) — either the desktop
  // replied, or the relay took custody of the frame and will hand it to the
  // desktop when it reconnects. Under a weak link the old fire-and-forget
  // `answer` would drop the frame, yet the card was removed optimistically — so
  // the desktop stayed blocked while the card vanished. Here a lost frame is
  // resent, and a final failure keeps the card so the boss can retry rather than
  // silently losing the answer.
  //
  // Note the relay-custody case is genuinely weaker than a desktop reply: the
  // card leaves this list before any session has acted on it. That is the right
  // trade — this phone's socket usually lives seconds, so demanding a desktop
  // round trip is what made answers unsendable while cards kept arriving.
  const [sending, setSending] = useState(false);
  const [sendFailed, setSendFailed] = useState(false);
  const submit = useCallback(
    (fields: Record<string, unknown>) => {
      if (!client || sending) return;
      setSending(true);
      setSendFailed(false);
      client
        .answerViaReq(decision.kind, decision.id, fields)
        .then(() => onAnswered(decision.id))
        .catch(() => {
          // No delivery verdict (or the desktop rejected): never mark answered
          // without confirmation — that is the bug. Keep the card and surface it.
          setSending(false);
          setSendFailed(true);
        });
    },
    [client, decision.kind, decision.id, onAnswered, sending],
  );

  return (
    <div className={`${styles.card} ${sending ? styles.cardBusy : ""}`} aria-busy={sending}>
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
            {t("会话")}
            <ChevronRight size={13} />
          </button>
        )}
      </div>
      {aiTitle && <div className={styles.aiTitle}>{aiTitle}</div>}
      {/* `in` rather than a field on the union: guard and permission prompts are
          not parkable (their timeout is a deny, not a pause), so they genuinely
          have no `parked` — the type says so, and it should keep saying so. */}
      {"parked" in req && req.parked === true && (
        <div className={styles.parkedBanner} role="status">
          <span className={styles.parkedBadge}>{t("已超时 · 会话已暂停")}</span>
          <span className={styles.parkedHint}>
            {t(
              "等待超时，Fleet 已经掐掉那一轮并把问题留了下来。回复后会带着你的答复唤醒会话继续。",
            )}
          </span>
        </div>
      )}
      {sending && (
        <div className={styles.sendingBanner} role="status">
          {t("发送中…等待桌面端确认收到")}
        </div>
      )}
      {sendFailed && (
        <div className={styles.sendFailBanner} role="alert">
          <CloudOff size={13} />
          <span>{t("发送失败,答复未送达,卡片已保留,请重试")}</span>
        </div>
      )}
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
          <ReactMarkdown
            remarkPlugins={mdRemarkPlugins}
            rehypePlugins={mdRehypePlugins}
            components={mermaidMarkdownComponents}
          >
            {state}
          </ReactMarkdown>
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
  const [showRaw, setShowRaw] = useState(false);
  const input = useMemo<Record<string, unknown>>(
    () =>
      request.toolInput && typeof request.toolInput === "object"
        ? (request.toolInput as Record<string, unknown>)
        : {},
    [request.toolInput],
  );
  const primary = useMemo(() => permissionPrimary(request.toolName, input), [request.toolName, input]);
  const rawJson = useMemo(() => {
    try {
      return JSON.stringify(request.toolInput, null, 2);
    } catch {
      return String(request.toolInput);
    }
  }, [request.toolInput]);
  const primaryKeyCount = primary.kind === "pattern" && primary.path ? 2 : 1;
  const hasExtraParams = primary.kind !== "json" && Object.keys(input).length > primaryKeyCount;
  return (
    <div>
      <div className={styles.toolName}>
        {t("工具：")}
        <code>{request.toolName}</code>
      </div>
      {primary.kind === "file" ? (
        <div className={styles.permFile}>
          <div className={styles.permFileName}>📄 {basename(primary.path)}</div>
          <div className={styles.permFilePath}>{primary.path}</div>
        </div>
      ) : primary.kind === "pattern" ? (
        <pre className={styles.command}>
          {primary.path ? `${primary.text}\n${t("位于")} ${primary.path}` : primary.text}
        </pre>
      ) : (
        primary.text &&
        primary.text !== "null" && <pre className={styles.command}>{truncate(primary.text, 6000)}</pre>
      )}
      {hasExtraParams && (
        <button type="button" className={styles.rawToggle} onClick={() => setShowRaw((v) => !v)}>
          {showRaw ? t("隐藏参数详情") : t("查看参数详情")}
        </button>
      )}
      {hasExtraParams && showRaw && <pre className={styles.command}>{truncate(rawJson, 6000)}</pre>}
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
          <ReactMarkdown
            remarkPlugins={mdRemarkPlugins}
            rehypePlugins={mdRehypePlugins}
            components={mermaidMarkdownComponents}
          >
            {content}
          </ReactMarkdown>
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
        {editing ? (
          t("放弃编辑")
        ) : (
          <>
            <Pencil size={13} />
            {t("编辑计划")}
          </>
        )}
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
export function slicePrecedingMessages(messages: RawMessage[]): { key: string; text: string }[] {
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
      // A failed ask call (is_error) was never actually answered — it errored
      // out before the user could respond, so it is NOT user input and must not
      // end the narration span.
      if (b.is_error) return false;
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
        {expanded ? (
          <>
            <ChevronDown size={14} />
            {t("收起提问前的说明")}
          </>
        ) : (
          <>
            <ChevronRight size={14} />
            {t("Agent 干活时还说了 {0} 段话", chunks.length)}
          </>
        )}
      </button>
      {expanded && (
        <div className={styles.precedingBody}>
          {chunks.map((c) => (
            <div key={c.key} className={styles.markdown}>
              <ReactMarkdown
                remarkPlugins={mdRemarkPlugins}
            rehypePlugins={mdRehypePlugins}
                components={mermaidMarkdownComponents}
              >
                {c.text}
              </ReactMarkdown>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── elicitation / fleet-ask ──────────────────────────────────────────────────

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
  // question text → selected option labels
  const [selections, setSelections] = useState<Record<string, string[]>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});
  // question text → user flipped a single-select into multi-select
  const [multiOverride, setMultiOverride] = useState<Record<string, boolean>>({});
  // question text → uploaded attachments appended to the answer as @path
  const [attachments, setAttachments] = useState<Record<string, Attachment[]>>({});
  // path → `blob:` URL of the bytes just picked, so the thumbnail is instant and
  // costs no round trip. A card lives and dies inside one page life, so unlike
  // the composer's draft there is no restore path to fall back from.
  const previews = useRef(new Map<string, string>());
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

  // Same head-off-screen problem one level down: the 上一题/下一题 buttons and the
  // step dots sit below a long question, so stepping leaves the next question's
  // header scrolled past. Snap back on every step change, like the card-level
  // effect above.
  useEffect(() => {
    if (window.scrollY === 0) return;
    window.scrollTo({ top: 0, behavior: "smooth" });
  }, [step]);

  /** Desktop `hasAnswer`: option / custom text / attachment / form-only,
   *  with required form fields filled. Gates Next/Submit instead of erroring
   *  after the fact. */
  const answeredQuestion = (q: ElicitationQuestion | FleetAskQuestion): boolean => {
    const picked = selections[q.question] ?? [];
    // Custom text is always live now (the "Other" box is always visible), so it
    // folds into the answer whenever non-empty — matching the desktop panel's
    // always-present composer instead of the old "其他…" toggle sentinel.
    const customText = (custom[q.question] ?? "").trim();
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
    // Just seed the always-visible "Other" box; its non-empty text is enough to
    // count as an answer (same as desktop's pencil → composer flow).
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
        for (const { previewUrl, ...a } of uploaded) {
          if (previewUrl) previews.current.set(a.path, previewUrl);
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
      const picked = selections[q.question] ?? [];
      // The "Other" box is always live; its non-empty text folds into the
      // answer alongside any picked options, mirroring the desktop composer.
      const customText = (custom[q.question] ?? "").trim();
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
            <ReactMarkdown
              remarkPlugins={mdRemarkPlugins}
            rehypePlugins={mdRehypePlugins}
              components={mermaidMarkdownComponents}
            >
              {stripTtsDivider(q.question)}
            </ReactMarkdown>
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
                    {/* No leading radio/checkbox mark — selection reads from the
                        card's coral border + soft fill, matching desktop's clean
                        option cards. */}
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
                        <Eye size={14} />
                      </button>
                    )}
                    <button
                      className={styles.optionSideBtn}
                      onClick={() => editToOther(q, o)}
                      aria-label={t("编辑此选项（复制到「其他」）")}
                    >
                      <Pencil size={14} />
                    </button>
                  </div>
                </div>
                {previewShown && o.preview && (
                  <div className={styles.optionPreview}>
                    <ReactMarkdown
                      remarkPlugins={mdRemarkPlugins}
            rehypePlugins={mdRehypePlugins}
                      components={mermaidMarkdownComponents}
                    >
                      {o.preview}
                    </ReactMarkdown>
                  </div>
                )}
              </div>
            );
          })}
          {/* Always-visible "Other" composer — mirrors the desktop panel: a
              labelled bordered box with an inline `+` attach button and a
              borderless textarea. Its non-empty text folds into the answer, so
              no explicit "其他…" toggle is needed. Option-less questions drop
              the label and let the box stand as the primary answer surface. */}
          <OtherComposer
            label={(q.options?.length ?? 0) > 0 ? t("其他") : undefined}
            placeholder={(q.options?.length ?? 0) > 0 ? t("自定义回答") : t("自由回答…")}
            value={custom[q.question] ?? ""}
            onChange={(v) => setCustom((p) => ({ ...p, [q.question]: v }))}
            question={q.question}
            attachments={attachments[q.question] ?? []}
            uploading={uploadingQ === q.question}
            client={client}
            previews={previews.current}
            onPick={(files) => void addQuestionFiles(q.question, files)}
            onRemove={(path) => {
              const url = previews.current.get(path);
              if (url) {
                URL.revokeObjectURL(url);
                previews.current.delete(path);
              }
              setAttachments((prev) => ({
                ...prev,
                [q.question]: (prev[q.question] ?? []).filter((a) => a.path !== path),
              }));
            }}
          />
          {(q.options?.length ?? 0) > 0 && !q.multiSelect && (
            <div className={styles.questionTools}>
              <button
                className={styles.toolToggle}
                data-active={multiOverride[q.question] === true}
                onClick={() => flipMultiOverride(q)}
              >
                {multiOverride[q.question] ? t("已改为多选") : t("改为多选")}
              </button>
            </div>
          )}
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

/** Always-visible "Other" composer — the desktop panel's clean equivalent of
 *  the old "其他…" toggle. A labelled bordered box holds attachment chips, an
 *  inline `+` picker (answers gain `@path` mentions), and a borderless textarea
 *  whose non-empty text folds into the answer. `label` is omitted for
 *  option-less questions, where the box is the primary answer surface. */
function OtherComposer({
  label,
  placeholder,
  value,
  onChange,
  question,
  attachments,
  uploading,
  client,
  previews,
  onPick,
  onRemove,
}: {
  label?: string;
  placeholder: string;
  value: string;
  onChange: (v: string) => void;
  question: string;
  attachments: Attachment[];
  uploading: boolean;
  client: RelayClient | null;
  previews: Map<string, string>;
  onPick: (files: FileList | null) => void;
  onRemove: (path: string) => void;
}) {
  const inputRef = useRef<HTMLInputElement | null>(null);
  return (
    <div className={styles.otherBlock}>
      {label && <span className={styles.otherLabel}>{label}</span>}
      <div className={styles.otherBox}>
        {/* Same tiles as the transcript and the composer: an image the user
            just attached is worth seeing before they send the answer, not just
            its filename. Non-images still render as a removable name chip. */}
        {attachments.length > 0 && (
          <div className={styles.otherChips}>
            <AttachmentThumbs
              paths={attachments.map((a) => a.path)}
              client={client}
              previews={previews}
              onRemove={onRemove}
              compact
            />
          </div>
        )}
        <div className={styles.otherRow}>
          <button
            className={styles.otherAttach}
            disabled={uploading}
            onClick={() => inputRef.current?.click()}
            aria-label={t("为「{0}」添加附件", question)}
          >
            <Plus size={14} />
          </button>
          <textarea
            className={styles.otherInput}
            placeholder={uploading ? t("上传中…") : placeholder}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            rows={2}
          />
        </div>
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
            placeholder={field.placeholder ?? undefined}
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
              {value === "true" && <Check size={12} />}
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
            placeholder={field.placeholder ?? undefined}
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    }
  }
}

// ── fleet-ask html / images ──────────────────────────────────────────────────

type AssetState =
  | { status: "loading" }
  | { status: "ok"; uri: string }
  | { status: "error" };

interface AssetsResult {
  states: Record<string, AssetState>;
  /** Re-fetch one asset by name — wired to the card's tap-to-retry affordance
   *  so a transient timeout (slow phone link) isn't a dead end. */
  retry: (name: string) => void;
}

function useAssets(
  names: string[],
  requestId: string,
  qidx: number,
  client: RelayClient | null,
): AssetsResult {
  const [states, setStates] = useState<Record<string, AssetState>>({});
  // A late resolve after the card unmounts is harmless but noisy; gate on it.
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const load = useCallback(
    (name: string) => {
      if (!client) return;
      setStates((p) => ({ ...p, [name]: { status: "loading" } }));
      fetchDecisionAsset(client, requestId, qidx, name)
        .then((a) => {
          if (mounted.current) {
            setStates((p) => ({
              ...p,
              [name]: { status: "ok", uri: `data:${a.mime};base64,${a.base64}` },
            }));
          }
        })
        .catch(() => {
          // Previously swallowed silently → the <img> spun forever on a slow
          // link (timeout) or a missing asset. Surface it so the card can retry.
          if (mounted.current) setStates((p) => ({ ...p, [name]: { status: "error" } }));
        });
    },
    [client, requestId, qidx],
  );

  useEffect(() => {
    for (const name of names) load(name);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestId, qidx, client, names.join("|")]);

  return { states, retry: load };
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
  const { states, retry } = useAssets(names, requestId, qidx, client);
  const { open: openLightbox } = useLightbox();
  const resolved = useMemo(() => {
    let out = html;
    for (const name of names) {
      const st = states[name];
      if (st?.status !== "ok") continue;
      out = out.split(`src="${name}"`).join(`src="${st.uri}"`);
      out = out.split(`src='${name}'`).join(`src='${st.uri}'`);
    }
    // Append the tap-to-zoom bridge so images inside the sandbox open the
    // app-level lightbox (the sandbox is opaque-origin, so it must postMessage).
    return out + IMG_ZOOM_INJECT;
  }, [html, names, states]);
  const failed = useMemo(
    () => names.filter((n) => states[n]?.status === "error"),
    [names, states],
  );
  // Until every referenced image resolves, the srcDoc still holds raw
  // `src="name"` refs the sandbox renders as broken-image glyphs (the torn
  // thumbnails in Boss's screenshot). Hold the iframe back behind a shimmer
  // until the assets settle; an errored asset stops being "pending" so the
  // iframe mounts and the tap-to-retry surface below can take over.
  const assetsPending = useMemo(
    () => names.some((n) => !states[n] || states[n].status === "loading"),
    [names, states],
  );

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
      const zoomSrc = parseImgZoom(e.data);
      if (zoomSrc) {
        openLightbox(zoomSrc);
        return;
      }
      const h = parseFrameHeight(e.data);
      if (h === null) return;
      setHeight((cur) => (cur === null || Math.abs(h - cur) > FRAME_DEAD_BAND ? h : cur));
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [resolved, openLightbox]);

  return (
    <>
      {assetsPending ? (
        <div
          className={`${styles.skeleton} ${styles.previewSkeleton}`}
          role="img"
          aria-label={t("加载预览…")}
        />
      ) : (
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
      )}
      {failed.length > 0 && (
        <button
          type="button"
          className={styles.assetError}
          onClick={() => failed.forEach(retry)}
        >
          {t("{0} 张图片加载失败，点按重试", String(failed.length))}
        </button>
      )}
    </>
  );
}

function ImageGallery({
  images,
  requestId,
  qidx,
  client,
}: {
  images: { name: string; caption?: string | null }[];
  requestId: string;
  qidx: number;
  client: RelayClient | null;
}) {
  const names = useMemo(() => images.map((i) => i.name), [images]);
  const { states, retry } = useAssets(names, requestId, qidx, client);
  const { open } = useLightbox();
  return (
    <div className={styles.gallery}>
      {images.map((img) => {
        const st = states[img.name];
        return (
          <figure key={img.name}>
            {st?.status === "ok" ? (
              <img
                src={st.uri}
                alt={img.caption ?? img.name}
                onClick={() => open(st.uri, img.caption ?? img.name)}
              />
            ) : st?.status === "error" ? (
              <button
                type="button"
                className={styles.assetError}
                onClick={() => retry(img.name)}
              >
                {t("图片加载失败，点按重试")}
              </button>
            ) : (
              <div
                className={`${styles.skeleton} ${styles.imgSkeleton}`}
                role="img"
                aria-label={t("加载图片…")}
              />
            )}
            {img.caption && <figcaption>{img.caption}</figcaption>}
          </figure>
        );
      })}
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
