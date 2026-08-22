import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { invoke } from "@tauri-apps/api/core";
import {
  resolveTheme,
  useDecisionStore,
  useSessionsStore,
  useUIStore,
} from "../store";
import { safeRemarkPlugins, safeRehypePlugins } from "../markdown/safeLinks";
import { normalizeSvgBlankLines, markdownUrlTransform } from "../markdown/plugins";
import { usePathMarkdown } from "../hooks/usePathLinks";
import { usePrecedingAgentMessages } from "../hooks/usePrecedingAgentMessages";
import type {
  DecisionHistoryRecord,
  ElicitationAttachment,
  ElicitationDecision,
  FleetAskDecision,
  FleetAskFormField,
  GuardDecision,
  PendingDecision,
  PermissionPromptDecision,
  PlanApprovalDecision,
} from "../types";
import { A2uiRenderCard } from "./A2uiRenderCard";
import { fileExtIcon } from "./blocks/Rail";
import { basename } from "./blocks/ToolUseBlock";
import { permissionPrimary } from "./permissionPrimary";
import { ChatComposer, type ChatComposerHandle } from "./ChatComposer";
import { SessionDetail } from "./SessionDetail";
import { StructuredCommandView } from "./StructuredCommandView";
import { useAutoFlip } from "./useAutoFlip";
import { decisionAssetUrl } from "../decisionAssets";
import { AutoHeightFrame } from "./AutoHeightFrame";
import { ReviewDocsColumn } from "./ReviewDocsColumn";
import styles from "./DecisionPanel.module.css";

function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}


// ── Guard card renderer ────────────────────────────────────────────────────

/**
 * A reusable subcommand is a bare lowercase word: git's `push`, docker's
 * `exec`, patchwright-cli's `eval`. We deliberately EXCLUDE not just flags
 * (`-s=clw`, `--verbose`) but also separated flag values, paths, URLs,
 * `KEY=VAL` and uppercase args — e.g. `curl -X POST https://…` has no real
 * subcommand, so its prefix should fall back to bare `curl` rather than the
 * un-reusable `curl POST` / `curl https://specific-url`. Mirrors the backend's
 * `looks_like_subcommand` so the remembered prefix actually matches later runs.
 */
function looksLikeSubcommand(tok: string): boolean {
  return /^[a-z][a-z0-9_-]*$/.test(tok);
}

/**
 * Prefix for a single AST leaf: `argv[0]` plus the first bare-word subcommand
 * after it (the "subcommand" in the `git push` / `npm test` / `patchwright-cli
 * eval` sense), skipping flags, flag values, paths and URLs in between. Falls
 * back to just `argv[0]` when there is no bare-word subcommand (e.g. `curl`,
 * `git -C /path` with no later subcommand).
 */
function computeLeafAllowPrefix(argv: string[]): string {
  const head = argv[0];
  if (!head) return "";
  const sub = argv.slice(1).find((t) => looksLikeSubcommand(t));
  return sub ? `${head} ${sub}` : head;
}

/**
 * Legacy fallback when the backend didn't ship a `structuredCommand` AST
 * (older RemoteBackend versions).  Same shape as the AST-driven path but
 * applied to the raw first line.
 */
function computeFallbackPrefix(command: string): string {
  const firstLine = command.split("\n")[0]?.trim() ?? "";
  if (!firstLine) return "";
  const tokens = firstLine.split(/\s+/).filter((t) => t.length > 0);
  return computeLeafAllowPrefix(tokens);
}

/**
 * One prefix per AST leaf that actually fired the audit and is not yet
 * covered by an existing allow rule.  Older `fleet guard` payloads ship
 * leaves without `triggering` / `already_allowed` set — when none of the
 * leaves carry `triggering=true`, fall back to "every leaf" so old desktop
 * + old CLI combinations keep working.
 *
 * When the structured AST itself is absent, fall back to a single prefix
 * derived from the raw command string.
 */
function computeGuardAllowPrefixes(req: GuardDecision["request"]): string[] {
  const view = req.structuredCommand;
  if (view && view.leaves.length > 0) {
    const anyTriggering = view.leaves.some((leaf) => leaf.triggering === true);
    const eligible = anyTriggering
      ? view.leaves.filter(
          (leaf) => leaf.triggering === true && leaf.already_allowed !== true,
        )
      : view.leaves; // legacy payload — preserve historical behaviour
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
    // All triggering leaves are already covered — don't fall back to the raw
    // command, because that would defeat the filter the user just enacted.
    if (anyTriggering) return [];
  }
  const fallback = computeFallbackPrefix(req.command);
  return fallback ? [fallback] : [];
}

/**
 * Shown on a card whose wait timed out. The card did not go away and the agent
 * did not carry on without an answer: Fleet interrupted that turn and is holding
 * the question. Submitting now resumes the session with the reply attached.
 */
export function ParkedBanner() {
  const { t } = useTranslation();
  return (
    <div className={styles.parked_banner} role="status">
      <span className={styles.parked_badge}>
        {t("parked.badge", "Timed out — session paused")}
      </span>
      <span className={styles.parked_hint}>
        {t(
          "parked.hint",
          "No answer arrived in time, so Fleet stopped this turn and kept the question. Answering resumes the session with your reply attached.",
        )}
      </span>
    </div>
  );
}

function GuardCard({ decision }: { decision: GuardDecision }) {
  const { t } = useTranslation();
  const { respond } = useDecisionStore();
  const req = decision.request;
  const mdComponents = usePathMarkdown(req.sessionId);

  const allowPrefixes = useMemo(() => computeGuardAllowPrefixes(req), [req]);
  const sourceTag = req.riskTags[0] ?? null;
  const [menuOpen, setMenuOpen] = useState(false);
  const [blockReason, setBlockReason] = useState("");
  const menuWrapRef = useRef<HTMLDivElement | null>(null);
  const menuPanelRef = useRef<HTMLDivElement | null>(null);
  // Decision cards sit anywhere in a scrolling list, so a card near the top of
  // the viewport would push this panel off screen if it always opened upward.
  const menuSide = useAutoFlip(menuOpen, "above", menuPanelRef, menuWrapRef);

  useEffect(() => {
    if (!menuOpen) return;
    const onDocClick = (e: MouseEvent) => {
      if (!menuWrapRef.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, [menuOpen]);

  const handleAllow = useCallback(
    () => respond(decision.id, true),
    [respond, decision.id],
  );
  const handleBlock = useCallback(
    () => respond(decision.id, false, null, blockReason),
    [respond, decision.id, blockReason],
  );
  const handleAlwaysAllowPrefix = useCallback(
    (prefix: string) => {
      setMenuOpen(false);
      respond(decision.id, true, { prefix, sourceTag });
    },
    [respond, decision.id, sourceTag],
  );

  return (
    <div className={styles.card}>
      <div className={styles.card_header}>
        <svg
          className={styles.card_icon}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
          <line x1="12" y1="9" x2="12" y2="13" />
          <line x1="12" y1="17" x2="12.01" y2="17" />
        </svg>
        <span className={styles.card_title}>
          {t("guard.title", "Critical Command Detected")}
        </span>
        {req.workspaceName && (
          <span className={styles.card_workspace}>{req.workspaceName}</span>
        )}
      </div>

      {req.aiTitle && (
        <div className={styles.card_subtitle}>{req.aiTitle}</div>
      )}

      <StructuredCommandView command={req.command} view={req.structuredCommand} />

      {req.riskTags.length > 0 && (
        <div className={styles.tags}>
          {req.riskTags.map((tag) => (
            <span key={tag} className={styles.tag}>{tag}</span>
          ))}
        </div>
      )}

      {(decision.analyzing || decision.analysis) && (
        <div className={`${styles.analysis} ${decision.analyzing ? styles.analysis_loading : ""}`}>
          {decision.analyzing
            ? t("guard.analyzing", "Analyzing command...")
            : <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>{normalizeSvgBlankLines(decision.analysis ?? "")}</ReactMarkdown>}
        </div>
      )}

      <div className={styles.block_reason_row}>
        <input
          type="text"
          className={styles.block_reason_input}
          placeholder={t(
            "guard.block_reason_placeholder",
            "Reason for AI (optional)",
          )}
          value={blockReason}
          onChange={(e) => setBlockReason(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              handleBlock();
            }
          }}
          aria-label={t(
            "guard.block_reason_placeholder",
            "Reason for AI (optional)",
          )}
        />
      </div>

      <div className={styles.actions}>
        <button className={`${styles.btn} ${styles.btn_allow}`} onClick={handleAllow}>
          {t("guard.allow", "Allow")}
        </button>
        {allowPrefixes.length === 1 && (
          <button
            className={`${styles.btn} ${styles.btn_always_allow}`}
            onClick={() => handleAlwaysAllowPrefix(allowPrefixes[0]!)}
            title={t("guard.always_allow_hint", "Future commands starting with {{prefix}} will be allowed without asking", { prefix: allowPrefixes[0] })}
          >
            {t("guard.always_allow", "Always allow")} <code>{allowPrefixes[0]}</code>
          </button>
        )}
        {allowPrefixes.length > 1 && (
          <div className={styles.always_allow_menu_wrap} ref={menuWrapRef}>
            <button
              className={`${styles.btn} ${styles.btn_always_allow}`}
              onClick={() => setMenuOpen((v) => !v)}
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              title={t("guard.always_allow_pick_hint", "Pick which command to always allow")}
            >
              {t("guard.always_allow_menu", "Always allow")} <span aria-hidden>▾</span>
            </button>
            {menuOpen && (
              <div
                className={`${styles.always_allow_menu_panel} ${
                  menuSide === "above" ? styles.menu_above : styles.menu_below
                }`}
                ref={menuPanelRef}
                role="menu"
              >
                <div className={styles.always_allow_menu_title}>
                  {t("guard.always_allow_pick_hint", "Pick which command to always allow")}
                </div>
                {allowPrefixes.map((p) => (
                  <button
                    key={p}
                    type="button"
                    role="menuitem"
                    className={styles.always_allow_menu_item}
                    onClick={() => handleAlwaysAllowPrefix(p)}
                  >
                    <code>{p}</code>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
        <button className={`${styles.btn} ${styles.btn_block}`} onClick={handleBlock}>
          {t("guard.block", "Block")}
        </button>
      </div>
    </div>
  );
}

// ── Permission-prompt card (headless native-permission bridge) ────────────

/**
 * A native Claude Code permission prompt routed from a headless session via
 * `--permission-prompt-tool` → `fleet__permission_prompt`. Layout mirrors the
 * guard card (allow / deny + optional deny reason), but the payload is a
 * tool name + its full input JSON rather than a shell command AST.
 */
function PermissionPromptCard({ decision }: { decision: PermissionPromptDecision }) {
  const { t } = useTranslation();
  const respondToPermissionPrompt = useDecisionStore((s) => s.respondToPermissionPrompt);
  const setPermissionPromptDenyReason = useDecisionStore(
    (s) => s.setPermissionPromptDenyReason,
  );
  const [showRaw, setShowRaw] = useState(false);
  const req = decision.request;

  const input = useMemo<Record<string, unknown>>(
    () =>
      req.toolInput && typeof req.toolInput === "object"
        ? (req.toolInput as Record<string, unknown>)
        : {},
    [req.toolInput],
  );
  const primary = useMemo(
    () => permissionPrimary(req.toolName, input),
    [req.toolName, input],
  );
  const rawJson = useMemo(() => {
    try {
      const text = JSON.stringify(req.toolInput ?? {}, null, 2);
      return text.length > 6000 ? `${text.slice(0, 6000)}\n…` : text;
    } catch {
      return String(req.toolInput);
    }
  }, [req.toolInput]);

  // The raw-params toggle is redundant when the primary display already *is* the
  // JSON fallback, or when the input has no fields beyond the one we lead with.
  const primaryKeyCount = primary.kind === "pattern" && primary.path ? 2 : 1;
  const hasExtraParams =
    primary.kind !== "json" && Object.keys(input).length > primaryKeyCount;

  const handleAllow = useCallback(
    () => respondToPermissionPrompt(decision.id, true),
    [respondToPermissionPrompt, decision.id],
  );
  const handleDeny = useCallback(
    () => respondToPermissionPrompt(decision.id, false),
    [respondToPermissionPrompt, decision.id],
  );

  return (
    <div className={styles.card}>
      <div className={styles.card_header}>
        <svg
          className={styles.card_icon}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
          <path d="M7 11V7a5 5 0 0 1 10 0v4" />
        </svg>
        <span className={styles.card_title}>
          {t("permission_prompt.title", "Permission Request")}
        </span>
        {req.workspaceName && (
          <span className={styles.card_workspace}>{req.workspaceName}</span>
        )}
      </div>

      {req.aiTitle && (
        <div className={styles.card_subtitle}>{req.aiTitle}</div>
      )}

      <div className={styles.tags}>
        <span className={styles.tag}>{req.toolName}</span>
      </div>

      {primary.kind === "file" ? (
        <div className={styles.perm_file}>
          <div className={styles.perm_file_head}>
            <span className={styles.perm_file_icon} aria-hidden>
              {fileExtIcon(primary.path)}
            </span>
            <span className={styles.perm_file_name}>{basename(primary.path)}</span>
          </div>
          <span className={styles.perm_file_path}>{primary.path}</span>
        </div>
      ) : primary.kind === "pattern" ? (
        <div className={styles.command}>
          {primary.path
            ? `${primary.text}\n${t("permission_prompt.in_path", "in")} ${primary.path}`
            : primary.text}
        </div>
      ) : (
        <div className={styles.command}>{primary.text}</div>
      )}

      {hasExtraParams && (
        <button
          type="button"
          className={styles.perm_raw_toggle}
          onClick={() => setShowRaw((v) => !v)}
        >
          {showRaw
            ? t("permission_prompt.hide_params", "隐藏参数详情")
            : t("permission_prompt.show_params", "查看参数详情")}
        </button>
      )}
      {hasExtraParams && showRaw && <div className={styles.command}>{rawJson}</div>}

      <div className={styles.block_reason_row}>
        <input
          type="text"
          className={styles.block_reason_input}
          placeholder={t(
            "permission_prompt.deny_reason_placeholder",
            "Reason for AI (optional)",
          )}
          value={decision.denyReason}
          onChange={(e) => setPermissionPromptDenyReason(decision.id, e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              handleDeny();
            }
          }}
          aria-label={t(
            "permission_prompt.deny_reason_placeholder",
            "Reason for AI (optional)",
          )}
        />
      </div>

      <div className={styles.actions}>
        <button className={`${styles.btn} ${styles.btn_allow}`} onClick={handleAllow}>
          {t("permission_prompt.allow", "Allow")}
        </button>
        <button className={`${styles.btn} ${styles.btn_block}`} onClick={handleDeny}>
          {t("permission_prompt.deny", "Deny")}
        </button>
      </div>
    </div>
  );
}

// ── Elicitation card renderer (multi-step wizard) ─────────────────────────

/**
 * The agent's plain-text narration since the user's last input, shown above a
 * question card. Collapsed by default to a single slim hint bar ("Agent said N
 * more things while working") so the question stays the focus; clicking the bar
 * expands an inner scrollable region with the full narration and the card grows
 * to fit. Renders nothing when there's no narration.
 */
function PrecedingAgentMessagesRegion({
  sessionId,
  requestId,
}: {
  sessionId: string;
  requestId: string;
}) {
  const { t } = useTranslation();
  const { messages, loading } = usePrecedingAgentMessages(sessionId, requestId);
  const mdComponents = usePathMarkdown(sessionId);
  const [expanded, setExpanded] = useState(false);

  // Collapse again whenever a fresh question arrives for this card so the next
  // decision opens focused on the question, not someone's old expanded state.
  useEffect(() => {
    setExpanded(false);
  }, [requestId]);

  if (loading || !messages.length) return null;

  return (
    <div className={`${styles.preceding} ${expanded ? styles.preceding_open : ""}`}>
      <button
        type="button"
        className={styles.preceding_toggle}
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        <span className={styles.preceding_toggle_label}>
          {expanded
            ? t("decision.preceding_label", "Agent said while working")
            : t("decision.preceding_hint", {
                n: messages.length,
                defaultValue: "Agent said {{n}} more while working",
              })}
        </span>
      </button>
      {expanded && (
        <div className={styles.preceding_body}>
          {messages.map((m, i) => (
            <div key={m.uuid ?? i} className={styles.preceding_msg}>
              <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>
                {normalizeSvgBlankLines(m.text)}
              </ReactMarkdown>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * Slim toggle bar sitting at the top of the card footer. Options always start
 * expanded; collapsing is a manual, per-question act for document-style cards
 * where the reader wants the body to take the full height. Collapsing hides the
 * option list / form fields / "Other" composer beneath it; the action buttons
 * (Decline / Back / Next / Submit) stay visible. When collapsed with answers
 * already picked, echoes a one-line summary so the current choice stays legible.
 */
function OptionsCollapseBar({
  collapsed,
  onToggle,
  count,
  summary,
}: {
  collapsed: boolean;
  onToggle: () => void;
  count: number;
  summary: string | null;
}) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      className={styles.options_collapse_bar}
      onClick={onToggle}
      aria-expanded={!collapsed}
    >
      <svg
        className={`${styles.preceding_chevron} ${!collapsed ? styles.preceding_chevron_open : ""}`}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <polyline points="6 9 12 15 18 9" />
      </svg>
      <span className={styles.options_collapse_label}>
        {collapsed
          ? count > 0
            ? t("decision.options_expand", {
                n: count,
                defaultValue: "Show options ({{n}})",
              })
            : t("decision.options_expand_plain", "Show answer box")
          : t("decision.options_collapse", "Hide options")}
      </span>
      {collapsed && summary && (
        <span className={styles.options_collapse_summary} title={summary}>
          {t("decision.options_selected", {
            s: summary,
            defaultValue: "Selected: {{s}}",
          })}
        </span>
      )}
    </button>
  );
}

function ElicitationCard({ decision, compact = false }: { decision: ElicitationDecision; compact?: boolean }) {
  const parked = decision.request.parked === true;
  const { t } = useTranslation();
  const mdComponents = usePathMarkdown(decision.request.sessionId);
  const {
    submitElicitation,
    declineElicitation,
    toggleElicitationOption,
    setElicitationCustomAnswer,
    setElicitationMultiSelectOverride,
    setElicitationStep,
    addElicitationAttachment,
    removeElicitationAttachment,
  } = useDecisionStore();
  const [attachError, setAttachError] = useState<string | null>(null);
  const errorDismissTimer = useRef<number | null>(null);
  const showAttachError = useCallback((msg: string) => {
    setAttachError(msg);
    if (errorDismissTimer.current) {
      window.clearTimeout(errorDismissTimer.current);
    }
    errorDismissTimer.current = window.setTimeout(() => {
      setAttachError(null);
      errorDismissTimer.current = null;
    }, 6000);
  }, []);
  useEffect(
    () => () => {
      if (errorDismissTimer.current) {
        window.clearTimeout(errorDismissTimer.current);
      }
    },
    [],
  );

  const { step, request, selections, customAnswers, multiSelectOverrides, attachments } = decision;
  const total = request.questions.length;
  const q = request.questions[step];
  const isLast = step === total - 1;

  // Effective multi-select: the question's own flag OR a user-forced override.
  const effectiveMulti = q.multiSelect || multiSelectOverrides[q.question] === true;
  const canToggleMode = !q.multiSelect; // Only allow override when question was originally single-select.

  const selected = selections[q.question] || [];
  const customText = customAnswers[q.question] || "";
  const questionAttachments = attachments[q.question] || [];
  const hasAnswer =
    selected.length > 0 || customText.trim().length > 0 || questionAttachments.length > 0;

  // Options always start expanded; collapsing is a manual act for the rare
  // document-style card. Reset when the wizard moves to a different question.
  const [optionsCollapsed, setOptionsCollapsed] = useState(false);
  useEffect(() => {
    setOptionsCollapsed(false);
  }, [q.question]);
  const collapsedSummary = useMemo(() => {
    const parts = [...selected, customText.trim()].filter((s) => s.length > 0);
    return parts.length > 0 ? parts.join(" · ") : null;
  }, [selected, customText]);

  const allAnswered = request.questions.every((qq) => {
    const sel = selections[qq.question] || [];
    const custom = customAnswers[qq.question]?.trim();
    const atts = attachments[qq.question] || [];
    return sel.length > 0 || (custom != null && custom.length > 0) || atts.length > 0;
  });

  const handleBack = useCallback(
    () => setElicitationStep(decision.id, step - 1),
    [setElicitationStep, decision.id, step],
  );
  const handleNext = useCallback(
    () => setElicitationStep(decision.id, step + 1),
    [setElicitationStep, decision.id, step],
  );
  const handleSubmit = useCallback(
    () => submitElicitation(decision.id),
    [submitElicitation, decision.id],
  );
  const handleDecline = useCallback(
    () => declineElicitation(decision.id),
    [declineElicitation, decision.id],
  );

  return (
    <div className={`${styles.card} ${styles.card_flex}`}>
      <div className={styles.card_scroll}>
      <PrecedingAgentMessagesRegion sessionId={request.sessionId} requestId={request.id} />
      <div className={styles.card_header}>
        <svg
          className={styles.card_icon_question}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <circle cx="12" cy="12" r="10" />
          <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
          <line x1="12" y1="17" x2="12.01" y2="17" />
        </svg>
        <span className={styles.card_title}>
          {t("elicitation.title", "Agent Question")}
        </span>
        {total > 1 && (
          <span className={styles.elicitation_step_badge}>
            {step + 1} / {total}
          </span>
        )}
        {canToggleMode && (
          <button
            type="button"
            className={`${styles.mode_toggle} ${effectiveMulti ? styles.mode_toggle_multi : ""}`}
            onClick={() =>
              setElicitationMultiSelectOverride(
                decision.id,
                q.question,
                !effectiveMulti,
              )
            }
            title={t("elicitation.mode_tooltip", "Switch between single/multi select")}
          >
            {effectiveMulti
              ? t("elicitation.mode_multi", "Multi")
              : t("elicitation.mode_single", "Single")}
          </button>
        )}
        {request.workspaceName && (
          <span className={styles.card_workspace}>{request.workspaceName}</span>
        )}
      </div>

      {request.aiTitle && (
        <div className={styles.card_subtitle}>{request.aiTitle}</div>
      )}

      {total > 1 && (
        <div className={styles.elicitation_dots}>
          {request.questions.map((qq, i) => {
            const answered =
              (selections[qq.question] || []).length > 0 ||
              (customAnswers[qq.question]?.trim().length ?? 0) > 0 ||
              (attachments[qq.question] || []).length > 0;
            return (
              <button
                key={i}
                className={`${styles.elicitation_dot} ${i === step ? styles.elicitation_dot_active : ""} ${answered && i !== step ? styles.elicitation_dot_done : ""}`}
                onClick={() => setElicitationStep(decision.id, i)}
              />
            );
          })}
        </div>
      )}

      <div className={styles.elicitation_question}>
        <div className={styles.elicitation_question_text}>
          {q.header && (
            <span className={styles.elicitation_header}>{q.header}</span>
          )}
          <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>{normalizeSvgBlankLines(q.question)}</ReactMarkdown>
        </div>
      </div>
      </div>

      {/* Always-visible footer — options / "Other" / actions stay reachable
          without scrolling (flex-none tail; the body scrolls in .card_scroll). */}
      <div className={styles.card_footer}>
        {/* Manual collapse is offered whenever there is more than one option to
            fold (default stays expanded); only a single-option card stays open. */}
        {q.options.length > 1 && (
          <OptionsCollapseBar
            collapsed={optionsCollapsed}
            onToggle={() => setOptionsCollapsed((v) => !v)}
            count={q.options.length}
            summary={collapsedSummary}
          />
        )}
        {(q.options.length <= 1 || !optionsCollapsed) && (
        <SharedOptionsBlock
          decisionId={decision.id}
          sessionId={decision.request.sessionId}
          question={q}
          compact={compact}
          effectiveMulti={effectiveMulti}
          selected={selected}
          onToggle={toggleElicitationOption}
          customText={customText}
          onCustomChange={(val) => setElicitationCustomAnswer(decision.id, q.question, val)}
          attachments={questionAttachments}
          onAddAttachment={async (path, name, fromClipboard, preview) => {
            try {
              await addElicitationAttachment(decision.id, q.question, path, name, fromClipboard, preview);
            } catch (e) {
              const detail = e instanceof Error ? e.message : String(e);
              showAttachError(
                `${t("elicitation.attach_failed", "Attachment upload failed")}: ${detail}`,
              );
            }
          }}
          onRemoveAttachment={(path) =>
            removeElicitationAttachment(decision.id, q.question, path)
          }
          onAttachmentError={showAttachError}
        />
        )}
        {attachError && (
          <div className={styles.elicitation_attach_error} role="alert">
            <span className={styles.elicitation_attach_error_text}>{attachError}</span>
            <button
              type="button"
              className={styles.elicitation_attach_error_dismiss}
              onClick={() => setAttachError(null)}
              aria-label={t("elicitation.attach_error_dismiss", "Dismiss")}
            >
              ×
            </button>
          </div>
        )}

        {parked && <ParkedBanner />}
        <div className={styles.actions}>
        <button
          className={`${styles.btn} ${styles.btn_secondary}`}
          onClick={handleDecline}
        >
          {parked ? t("parked.discard", "Discard") : t("elicitation.decline", "Decline")}
        </button>
        <div className={styles.actions_spacer} />
        {step > 0 && (
          <button
            className={`${styles.btn} ${styles.btn_secondary}`}
            onClick={handleBack}
          >
            {t("elicitation.back", "Back")}
          </button>
        )}
        {isLast ? (
          <button
            className={`${styles.btn} ${styles.btn_allow}`}
            onClick={handleSubmit}
            disabled={!allAnswered}
          >
            {parked ? t("parked.submit", "Answer & wake the session") : t("elicitation.submit", "Submit")}
          </button>
        ) : (
          <button
            className={`${styles.btn} ${styles.btn_allow}`}
            onClick={handleNext}
            disabled={!hasAnswer}
          >
            {t("elicitation.next", "Next")}
          </button>
        )}
        </div>
      </div>
    </div>
  );
}

// Structural question shape that both ElicitationQuestion and FleetAskQuestion
// satisfy. SharedOptionsBlock reads `question` (used as the store-map key),
// `multiSelect`, and the options array — nothing else. Keeping the type local
// means we don't have to widen the option shape across both decision flows.
interface SharedOptionsQuestion {
  question: string;
  multiSelect: boolean;
  options: Array<{ label: string; description: string; preview?: string | null }>;
}

// Renders the option list + "Other" input. Splits into side-by-side layout
// when any option carries a preview (single-select only, per AskUserQuestion spec).
// Shared between ElicitationCard (v1) and FleetAskCard (v2) — both pass their
// own store actions in via callbacks, so the same UI surface drives both.
function SharedOptionsBlock({
  decisionId,
  sessionId,
  question,
  compact,
  effectiveMulti,
  selected,
  onToggle,
  customText,
  onCustomChange,
  attachments,
  onAddAttachment,
  onRemoveAttachment,
  onAttachmentError,
}: {
  decisionId: string;
  sessionId: string;
  question: SharedOptionsQuestion;
  compact: boolean;
  effectiveMulti: boolean;
  selected: string[];
  onToggle: (id: string, questionText: string, label: string, multiSelect: boolean) => void;
  customText: string;
  onCustomChange: (val: string) => void;
  attachments: ElicitationAttachment[];
  onAddAttachment: (
    path: string,
    name: string,
    fromClipboard?: boolean,
    preview?: { previewUrl: string; width: number; height: number },
  ) => void | Promise<void>;
  onRemoveAttachment: (path: string) => void;
  onAttachmentError: (msg: string) => void;
}) {
  const mdComponents = usePathMarkdown(sessionId);
  const { t } = useTranslation();
  const composerRef = useRef<ChatComposerHandle | null>(null);
  // Preview side-by-side layout only applies when question is single-select per
  // the AskUserQuestion spec. User-forced multi mode falls back to list layout.
  const hasPreview = useMemo(
    () => !effectiveMulti && question.options.some((o) => o.preview),
    [effectiveMulti, question],
  );
  const firstWithPreview = useMemo(
    () => question.options.find((o) => o.preview)?.label ?? question.options[0]?.label ?? "",
    [question.options],
  );
  const [focusedLabel, setFocusedLabel] = useState<string>(firstWithPreview);
  useEffect(() => {
    setFocusedLabel(firstWithPreview);
  }, [firstWithPreview]);

  const focusedPreview = question.options.find((o) => o.label === focusedLabel)?.preview;

  // Lite mode: push preview into a floating Tauri subwindow instead of the
  // inline grid, so the narrow main window isn't split in half. Normal mode
  // keeps the side-by-side layout and leaves the subwindow untouched.
  useEffect(() => {
    if (!compact) return;
    if (hasPreview) {
      const theme = resolveTheme(useUIStore.getState().theme);
      invoke("open_preview_window", {
        markdown: focusedPreview ?? "",
        title: focusedLabel || null,
        theme,
      }).catch(() => {});
    } else {
      invoke("close_preview_window").catch(() => {});
    }
  }, [compact, hasPreview, focusedPreview, focusedLabel]);

  // Tear down the subwindow when the card unmounts (decision resolved, tab
  // switched, or user exited lite mode). Only relevant in compact mode.
  useEffect(() => {
    if (!compact) return;
    return () => {
      invoke("close_preview_window").catch(() => {});
    };
  }, [compact]);

  const list = (
    <div className={styles.elicitation_options}>
      {question.options.map((opt) => {
        const isSelected = selected.includes(opt.label);
        const isFocused = hasPreview && opt.label === focusedLabel;
        const handleEdit = (e: React.MouseEvent) => {
          e.stopPropagation();
          const seed = opt.description
            ? `${opt.label} — ${opt.description}`
            : opt.label;
          onCustomChange(seed);
          // Focus the Other composer so the user can start editing immediately.
          requestAnimationFrame(() => {
            composerRef.current?.focus();
            composerRef.current?.setSelectionAtEnd();
          });
        };
        return (
          <div
            key={opt.label}
            className={`${styles.elicitation_option_row} ${isSelected ? styles.elicitation_option_row_selected : ""}`}
          >
            <button
              type="button"
              className={`${styles.elicitation_option} ${isSelected ? styles.elicitation_option_selected : ""} ${isFocused ? styles.elicitation_option_focused : ""}`}
              onClick={() =>
                onToggle(decisionId, question.question, opt.label, effectiveMulti)
              }
              onMouseEnter={hasPreview ? () => setFocusedLabel(opt.label) : undefined}
              onFocus={hasPreview ? () => setFocusedLabel(opt.label) : undefined}
            >
              <span className={styles.elicitation_option_label}>{opt.label}</span>
              {opt.description && (
                <span className={styles.elicitation_option_desc}>{opt.description}</span>
              )}
            </button>
            <button
              type="button"
              className={styles.elicitation_option_edit}
              onClick={handleEdit}
              title={t("elicitation.edit_option", "Edit this option (copy to Other)")}
              aria-label={t("elicitation.edit_option", "Edit this option (copy to Other)")}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M12 20h9" />
                <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z" />
              </svg>
            </button>
          </div>
        );
      })}

      <div className={styles.elicitation_other_block}>
        <span className={styles.elicitation_option_label}>
          {t("elicitation.other", "Other")}
        </span>
        <ChatComposer
          ref={composerRef}
          value={customText}
          onChange={onCustomChange}
          attachments={attachments}
          onAddAttachment={(s) =>
            onAddAttachment(s.path, s.name, s.fromClipboard, s.preview)
          }
          onRemoveAttachment={onRemoveAttachment}
          onAttachmentError={onAttachmentError}
          placeholder={t("elicitation.other_placeholder", "Type your answer…")}
          wikiMentions
        />
      </div>
    </div>
  );

  // In lite mode the preview lives in a floating subwindow — don't split the
  // panel in half. In normal mode keep the side-by-side grid as before.
  if (!hasPreview || compact) return list;
  return (
    <div className={styles.elicitation_options_with_preview}>
      {list}
      <div className={styles.elicitation_preview}>
        {focusedPreview ? (
          <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>{normalizeSvgBlankLines(focusedPreview)}</ReactMarkdown>
        ) : null}
      </div>
    </div>
  );
}

// ── Plan-approval card renderer ─────────────────────────────────────────

function PlanApprovalCard({ decision }: { decision: PlanApprovalDecision }) {
  const { t } = useTranslation();
  const mdComponents = usePathMarkdown(decision.request.sessionId);
  const { approvePlan, rejectPlan, setPlanEditedText, setPlanFeedback } = useDecisionStore();
  const [editing, setEditing] = useState(false);
  const [rejectMode, setRejectMode] = useState(false);
  const req = decision.request;

  const handleApprove = useCallback(
    () => approvePlan(decision.id, decision.editedPlan),
    [approvePlan, decision.id, decision.editedPlan],
  );
  const handleReject = useCallback(
    () => rejectPlan(decision.id, decision.feedback),
    [rejectPlan, decision.id, decision.feedback],
  );
  const handleStartEdit = useCallback(() => {
    if (decision.editedPlan === null) {
      setPlanEditedText(decision.id, req.planContent);
    }
    setEditing(true);
  }, [decision.editedPlan, decision.id, req.planContent, setPlanEditedText]);
  const handleCancelEdit = useCallback(() => {
    setPlanEditedText(decision.id, null);
    setEditing(false);
  }, [decision.id, setPlanEditedText]);

  return (
    <div className={styles.card}>
      <div className={styles.card_header}>
        <svg
          className={styles.card_icon_plan}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M9 11l3 3L22 4" />
          <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
        </svg>
        <span className={styles.card_title}>
          {t("planApproval.title", "Plan ready for approval")}
        </span>
        {req.workspaceName && (
          <span className={styles.card_workspace}>{req.workspaceName}</span>
        )}
      </div>

      {req.aiTitle && <div className={styles.card_subtitle}>{req.aiTitle}</div>}

      {req.planFilePath && (
        <div className={styles.plan_file_path}>{req.planFilePath}</div>
      )}

      {editing ? (
        <textarea
          className={styles.plan_textarea}
          value={decision.editedPlan ?? req.planContent}
          onChange={(e) => setPlanEditedText(decision.id, e.target.value)}
        />
      ) : (
        <div className={styles.plan_content}>
          <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>
            {normalizeSvgBlankLines(decision.editedPlan ?? req.planContent)}
          </ReactMarkdown>
        </div>
      )}

      {rejectMode && (
        <textarea
          className={styles.plan_feedback}
          value={decision.feedback}
          placeholder={t("planApproval.feedbackPlaceholder", "Leave feedback for the agent…")}
          onChange={(e) => setPlanFeedback(decision.id, e.target.value)}
        />
      )}

      {decision.request.parked === true && <ParkedBanner />}
      <div className={styles.actions}>
        {editing ? (
          <>
            <button
              className={`${styles.btn} ${styles.btn_secondary}`}
              onClick={handleCancelEdit}
            >
              {t("planApproval.cancelEdit", "Cancel edit")}
            </button>
            <div className={styles.actions_spacer} />
            <button
              className={`${styles.btn} ${styles.btn_allow}`}
              onClick={handleApprove}
            >
              {t("planApproval.approveEdited", "Approve edited")}
            </button>
          </>
        ) : rejectMode ? (
          <>
            <button
              className={`${styles.btn} ${styles.btn_secondary}`}
              onClick={() => setRejectMode(false)}
            >
              {t("planApproval.backToPlan", "Back")}
            </button>
            <div className={styles.actions_spacer} />
            <button
              className={`${styles.btn} ${styles.btn_block}`}
              onClick={handleReject}
            >
              {t("planApproval.rejectConfirm", "Reject plan")}
            </button>
          </>
        ) : (
          <>
            <button
              className={`${styles.btn} ${styles.btn_block}`}
              onClick={() => setRejectMode(true)}
            >
              {t("planApproval.reject", "Reject")}
            </button>
            <div className={styles.actions_spacer} />
            <button
              className={`${styles.btn} ${styles.btn_edit}`}
              onClick={handleStartEdit}
            >
              {t("planApproval.edit", "Edit")}
            </button>
            <button
              className={`${styles.btn} ${styles.btn_allow}`}
              onClick={handleApprove}
            >
              {t("planApproval.approve", "Approve")}
            </button>
          </>
        )}
      </div>
    </div>
  );
}

// ── fleet__ask card renderer (MCP-tool variant) ─────────────────────────
//
// Mirrors the elicitation card: stepped questions, header chip, workspace +
// AI title chips. Extends it with two optional render hooks per the P3 plan:
//   * `html` field non-empty → sandboxed <AutoHeightFrame> between the question
//     body and the answer controls. No same-origin, no forms, no top-navigation,
//     no popups; scripts are allowed solely so the opaque-origin document can
//     postMessage its height back (see AutoHeightFrame). The agent gets a
//     self-sizing preview without a route into the app's DOM.
//   * `formFields` non-empty → dynamic controls dispatched on `kind`
//     (text / textarea / number / select / radio / checkbox).
// `options` still renders the existing button grid. The three sections compose
// freely — a single question can carry any subset.

// Shared by both preview paths (served index.html vs inline srcDoc) so a card
// looks the same whether or not it carries images.
const FLEET_ASK_FRAME_STYLE = {
  width: "100%",
  border: "1px solid var(--decision-card-border, #ccc)",
  borderRadius: "0.4rem",
  background: "#fff",
} as const;


function FleetAskFormFieldRow({
  decisionId,
  field,
  value,
  onChange,
}: {
  decisionId: string;
  field: FleetAskFormField;
  value: string;
  onChange: (val: string) => void;
}) {
  const { t } = useTranslation();
  const id = `fa-${decisionId}-${field.name}`;
  const placeholder = field.placeholder ?? "";
  switch (field.kind) {
    case "textarea":
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
          </label>
          <textarea
            id={id}
            value={value}
            placeholder={placeholder}
            rows={4}
            onChange={(e) => onChange(e.target.value)}
            style={{ width: "100%", resize: "vertical" }}
          />
        </div>
      );
    case "number":
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
          </label>
          <input
            id={id}
            type="number"
            value={value}
            placeholder={placeholder}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    case "select":
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
          </label>
          <select id={id} value={value} onChange={(e) => onChange(e.target.value)}>
            <option value="" disabled>
              {placeholder || t("fleet_ask.select_placeholder", "Select…")}
            </option>
            {(field.options ?? []).map((opt) => (
              <option key={opt} value={opt}>
                {opt}
              </option>
            ))}
          </select>
        </div>
      );
    case "radio":
      return (
        <div className={styles.elicitation_other_block}>
          <span className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
          </span>
          {(field.options ?? []).map((opt) => (
            <label key={opt} style={{ display: "block" }}>
              <input
                type="radio"
                name={id}
                value={opt}
                checked={value === opt}
                onChange={(e) => onChange(e.target.value)}
              />{" "}
              {opt}
            </label>
          ))}
        </div>
      );
    case "checkbox": {
      // Checkbox semantics: single boolean. Serialised as "true" / "false" in
      // the answers map so the agent gets a stable string regardless of
      // language. Multi-checkbox groups should use kind="radio" with multiple
      // options or compose multiple checkbox fields — keeping this kind
      // boolean keeps the wire shape predictable.
      const checked = value === "true";
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} style={{ display: "flex", alignItems: "center", gap: "0.4rem" }}>
            <input
              id={id}
              type="checkbox"
              checked={checked}
              onChange={(e) => onChange(e.target.checked ? "true" : "false")}
            />
            <span>
              {field.label}
              {field.required && <span aria-hidden> *</span>}
            </span>
          </label>
        </div>
      );
    }
    case "date":
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
          </label>
          <input
            id={id}
            type="date"
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    case "datetime":
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
          </label>
          <input
            id={id}
            type="datetime-local"
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    case "time":
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
          </label>
          <input
            id={id}
            type="time"
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
    case "range": {
      // HTML5 range never renders the numeric value itself — surface it next
      // to the label so the user (and screenshot reviewers) can read what's
      // selected. Defaults match the HTML5 spec (0–100, step 1).
      const min = field.min ?? 0;
      const max = field.max ?? 100;
      const step = field.step ?? 1;
      const display = value === "" ? String(field.default ?? min) : value;
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
            <span style={{ marginLeft: "0.6rem", opacity: 0.7 }}>{display}</span>
          </label>
          <input
            id={id}
            type="range"
            min={min}
            max={max}
            step={step}
            value={display}
            onChange={(e) => onChange(e.target.value)}
            style={{ width: "100%" }}
          />
        </div>
      );
    }
    case "text":
    default:
      return (
        <div className={styles.elicitation_other_block}>
          <label htmlFor={id} className={styles.elicitation_option_label}>
            {field.label}
            {field.required && <span aria-hidden> *</span>}
          </label>
          <input
            id={id}
            type="text"
            value={value}
            placeholder={placeholder}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
      );
  }
}

export function FleetAskCard({
  decision,
  compact = false,
}: {
  decision: FleetAskDecision;
  compact?: boolean;
}) {
  const parked = decision.request.parked === true;
  const { t } = useTranslation();
  const mdComponents = usePathMarkdown(decision.request.sessionId);
  const {
    submitFleetAsk,
    cancelFleetAsk,
    toggleFleetAskOption,
    setFleetAskCustomAnswer,
    setFleetAskFormAnswer,
    setFleetAskStep,
    setFleetAskMultiSelectOverride,
    addFleetAskAttachment,
    removeFleetAskAttachment,
  } = useDecisionStore();
  const [attachError, setAttachError] = useState<string | null>(null);
  const errorDismissTimer = useRef<number | null>(null);
  const showAttachError = useCallback((msg: string) => {
    setAttachError(msg);
    if (errorDismissTimer.current) {
      window.clearTimeout(errorDismissTimer.current);
    }
    errorDismissTimer.current = window.setTimeout(() => {
      setAttachError(null);
      errorDismissTimer.current = null;
    }, 6000);
  }, []);
  useEffect(
    () => () => {
      if (errorDismissTimer.current) {
        window.clearTimeout(errorDismissTimer.current);
      }
    },
    [],
  );

  const {
    step,
    request,
    selections,
    customAnswers,
    formAnswers,
    multiSelectOverrides,
    attachments,
  } = decision;
  const total = request.questions.length;
  const q = request.questions[step];
  const isLast = step === total - 1;

  const opts = q.options ?? [];
  const formFields = q.formFields ?? [];
  const selected = selections[q.question] || [];
  const customText = customAnswers[q.question] || "";
  const questionAttachments = attachments[q.question] || [];

  // Mirror v1: user can locally widen a single-select question to multi-select.
  const effectiveMulti = q.multiSelect || multiSelectOverrides[q.question] === true;
  const canToggleMode = !q.multiSelect && opts.length > 0;

  // Options always start expanded; collapsing is a manual act for the rare
  // document-style card. Reset when the wizard moves to a different question.
  const [optionsCollapsed, setOptionsCollapsed] = useState(false);
  useEffect(() => {
    setOptionsCollapsed(false);
  }, [q.question]);
  const collapsedSummary = useMemo(() => {
    const parts = [...selected, customText.trim()].filter((s) => s.length > 0);
    return parts.length > 0 ? parts.join(" · ") : null;
  }, [selected, customText]);

  // Whether a question is "complete enough" to advance. Required form fields
  // plus at least one of (option picked / custom typed / attachment / form-only).
  const requiredFormFieldsFilled = formFields.every((f) => {
    if (!f.required) return true;
    const v = formAnswers[f.name];
    return v !== undefined && v !== "";
  });
  const hasOptionPick =
    opts.length === 0 ||
    selected.length > 0 ||
    customText.trim().length > 0 ||
    questionAttachments.length > 0;
  const hasAnswer = requiredFormFieldsFilled && hasOptionPick;

  const allAnswered = request.questions.every((qq) => {
    const qOpts = qq.options ?? [];
    const qFormFields = qq.formFields ?? [];
    const sel = selections[qq.question] || [];
    const custom = customAnswers[qq.question]?.trim() ?? "";
    const atts = attachments[qq.question] || [];
    const optsOk =
      qOpts.length === 0 || sel.length > 0 || custom.length > 0 || atts.length > 0;
    const formOk = qFormFields.every((f) => {
      if (!f.required) return true;
      const v = formAnswers[f.name];
      return v !== undefined && v !== "";
    });
    return optsOk && formOk;
  });

  const handleBack = useCallback(
    () => setFleetAskStep(decision.id, step - 1),
    [setFleetAskStep, decision.id, step],
  );
  const handleNext = useCallback(
    () => setFleetAskStep(decision.id, step + 1),
    [setFleetAskStep, decision.id, step],
  );
  const handleSubmit = useCallback(
    () => submitFleetAsk(decision.id),
    [submitFleetAsk, decision.id],
  );
  const handleCancel = useCallback(
    () => cancelFleetAsk(decision.id),
    [cancelFleetAsk, decision.id],
  );

  return (
    <div className={`${styles.card} ${styles.card_flex}`}>
      <div className={styles.card_scroll}>
      <PrecedingAgentMessagesRegion sessionId={request.sessionId} requestId={request.id} />
      <div className={styles.card_header}>
        <svg
          className={styles.card_icon_question}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <circle cx="12" cy="12" r="10" />
          <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
          <line x1="12" y1="17" x2="12.01" y2="17" />
        </svg>
        <span className={styles.card_title}>
          {t("fleet_ask.title", "Agent Question")}
        </span>
        {total > 1 && (
          <span className={styles.elicitation_step_badge}>
            {step + 1} / {total}
          </span>
        )}
        {canToggleMode && (
          <button
            type="button"
            className={`${styles.mode_toggle} ${effectiveMulti ? styles.mode_toggle_multi : ""}`}
            onClick={() =>
              setFleetAskMultiSelectOverride(
                decision.id,
                q.question,
                !effectiveMulti,
              )
            }
            title={t("fleet_ask.mode_tooltip", "Switch between single/multi select")}
          >
            {effectiveMulti
              ? t("fleet_ask.mode_multi", "Multi")
              : t("fleet_ask.mode_single", "Single")}
          </button>
        )}
        {request.workspaceName && (
          <span className={styles.card_workspace}>{request.workspaceName}</span>
        )}
      </div>

      {request.aiTitle && (
        <div className={styles.card_subtitle}>{request.aiTitle}</div>
      )}

      {total > 1 && (
        <div className={styles.elicitation_dots}>
          {request.questions.map((qq, i) => {
            const sel = selections[qq.question] || [];
            const custom = customAnswers[qq.question]?.trim() ?? "";
            const atts = attachments[qq.question] || [];
            const answered = sel.length > 0 || custom.length > 0 || atts.length > 0;
            return (
              <button
                key={i}
                className={`${styles.elicitation_dot} ${i === step ? styles.elicitation_dot_active : ""} ${answered && i !== step ? styles.elicitation_dot_done : ""}`}
                onClick={() => setFleetAskStep(decision.id, i)}
              />
            );
          })}
        </div>
      )}

      <div className={styles.elicitation_question}>
        <div className={styles.elicitation_question_text}>
          {q.header && (
            <span className={styles.elicitation_header}>{q.header}</span>
          )}
          <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>{normalizeSvgBlankLines(q.question)}</ReactMarkdown>
        </div>

        {q.images && q.images.length > 0 ? (
          // Image-bearing card: load the served index.html (agent html or auto
          // gallery) via the fleet-decision:// protocol so <img src="name">
          // resolves to the copied files — no base64 in the tool call.
          <AutoHeightFrame
            title={`fleet-ask-html-${decision.id}-${step}`}
            src={decisionAssetUrl(decision.id, `q${step}`)}
            minHeight={200}
            style={FLEET_ASK_FRAME_STYLE}
          />
        ) : (
          q.html && (
            <AutoHeightFrame
              title={`fleet-ask-html-${decision.id}-${step}`}
              srcDoc={q.html}
              minHeight={200}
              style={FLEET_ASK_FRAME_STYLE}
            />
          )
        )}
      </div>
      </div>

      {/* Always-visible footer — form fields / options / "Other" / actions stay
          reachable without scrolling (flex-none tail; body scrolls in .card_scroll). */}
      <div className={styles.card_footer}>
        {/* Same rule as elicitation: offer the collapse chevron whenever there is
            more than one option/form-field to fold (default stays expanded). */}
        {opts.length + formFields.length > 1 && (
          <OptionsCollapseBar
            collapsed={optionsCollapsed}
            onToggle={() => setOptionsCollapsed((v) => !v)}
            count={opts.length + formFields.length}
            summary={collapsedSummary}
          />
        )}
        {(opts.length + formFields.length <= 1 || !optionsCollapsed) && formFields.length > 0 && (
          <div className={styles.elicitation_options}>
            {formFields.map((f) => (
              <FleetAskFormFieldRow
                key={f.name}
                decisionId={decision.id}
                field={f}
                value={formAnswers[f.name] ?? ""}
                onChange={(v) => setFleetAskFormAnswer(decision.id, f.name, v)}
              />
            ))}
          </div>
        )}

        {/* Render whenever expanded — even with zero options — so the free-text
            "Other" composer (which lives inside SharedOptionsBlock, outside its
            options .map) is available on option-less fleet__ask cards. With an
            empty options array SharedOptionsBlock renders just the Other input.
            Only the user's collapse toggle hides it. */}
        {(opts.length + formFields.length <= 1 || !optionsCollapsed) && (
          <SharedOptionsBlock
            decisionId={decision.id}
            sessionId={decision.request.sessionId}
            question={{
              question: q.question,
              multiSelect: q.multiSelect,
              options: opts,
            }}
            compact={compact}
            effectiveMulti={effectiveMulti}
            selected={selected}
            onToggle={toggleFleetAskOption}
            customText={customText}
            onCustomChange={(val) =>
              setFleetAskCustomAnswer(decision.id, q.question, val)
            }
            attachments={questionAttachments}
            onAddAttachment={async (path, name, fromClipboard, preview) => {
              try {
                await addFleetAskAttachment(
                  decision.id,
                  q.question,
                  path,
                  name,
                  fromClipboard,
                  preview,
                );
              } catch (e) {
                const detail = e instanceof Error ? e.message : String(e);
                showAttachError(
                  `${t("fleet_ask.attach_failed", "Attachment upload failed")}: ${detail}`,
                );
              }
            }}
            onRemoveAttachment={(path) =>
              removeFleetAskAttachment(decision.id, q.question, path)
            }
            onAttachmentError={showAttachError}
          />
        )}
        {attachError && (
          <div className={styles.elicitation_attach_error} role="alert">
            <span className={styles.elicitation_attach_error_text}>{attachError}</span>
            <button
              type="button"
              className={styles.elicitation_attach_error_dismiss}
              onClick={() => setAttachError(null)}
              aria-label={t("fleet_ask.attach_error_dismiss", "Dismiss")}
            >
              ×
            </button>
          </div>
        )}

        {parked && <ParkedBanner />}
        <div className={styles.actions}>
        <button
          className={`${styles.btn} ${styles.btn_secondary}`}
          onClick={handleCancel}
        >
          {parked ? t("parked.discard", "Discard") : t("fleet_ask.cancel", "Cancel")}
        </button>
        <div className={styles.actions_spacer} />
        {step > 0 && (
          <button
            className={`${styles.btn} ${styles.btn_secondary}`}
            onClick={handleBack}
          >
            {t("fleet_ask.back", "Back")}
          </button>
        )}
        {isLast ? (
          <button
            className={`${styles.btn} ${styles.btn_allow}`}
            onClick={handleSubmit}
            disabled={!allAnswered}
          >
            {parked ? t("parked.submit", "Answer & wake the session") : t("fleet_ask.submit", "Submit")}
          </button>
        ) : (
          <button
            className={`${styles.btn} ${styles.btn_allow}`}
            onClick={handleNext}
            disabled={!hasAnswer}
          >
            {t("fleet_ask.next", "Next")}
          </button>
        )}
        </div>
      </div>
    </div>
  );
}

// ── Card dispatcher ──────────────────────────────────────────────────────

function DecisionCard({ decision, compact }: { decision: PendingDecision; compact: boolean }) {
  switch (decision.kind) {
    case "guard":
      return <GuardCard decision={decision} />;
    case "elicitation":
      return <ElicitationCard decision={decision} compact={compact} />;
    case "fleet-ask":
      return <FleetAskCard decision={decision} compact={compact} />;
    case "a2ui-render":
      return <A2uiRenderCard decision={decision} />;
    case "plan-approval":
      return <PlanApprovalCard decision={decision} />;
    case "permission-prompt":
      return <PermissionPromptCard decision={decision} />;
    default:
      return null;
  }
}

// ── Past-history handle (vertical tab toggling the inline SessionDetail) ──

function PastHistoryStrip({
  sessionId,
  expanded,
  onToggle,
}: {
  sessionId: string;
  expanded: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  const [records, setRecords] = useState<DecisionHistoryRecord[]>([]);

  useEffect(() => {
    if (!sessionId) {
      setRecords([]);
      return;
    }
    let cancelled = false;
    invoke<DecisionHistoryRecord[]>("list_session_decisions", {
      sessionId,
      jsonlPath: null,
    })
      .then((r) => {
        if (!cancelled) setRecords(r ?? []);
      })
      .catch(() => {
        if (!cancelled) setRecords([]);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  // Always render the handle (as long as the parent has a session): it toggles
  // the inline SessionDetail column, which is useful even on the very first
  // card of a session when no *prior* decision has been logged yet. The count
  // badge only shows once there is at least one past decision to count.
  const fullTitle = t(
    "decision_panel.history_title",
    "Recent in this session",
  );

  return (
    <button
      type="button"
      className={`${styles.history_handle} ${expanded ? styles.history_handle_open : ""}`}
      onClick={onToggle}
      aria-expanded={expanded}
      aria-label={fullTitle}
      title={fullTitle}
    >
      <svg
        className={styles.history_handle_icon}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="9" />
        <path d="M12 7v5l3 2" />
      </svg>
      {records.length > 0 && (
        <span className={styles.history_handle_count}>{records.length}</span>
      )}
    </button>
  );
}

// ── Tab label helper ─────────────────────────────────────────────────────

function tabLabel(d: PendingDecision): string {
  if (d.kind === "guard") {
    return d.request.toolName || "Guard";
  }
  if (d.kind === "plan-approval") {
    return d.request.aiTitle || "Plan";
  }

  if (d.kind === "a2ui-render") {
    const text = d.request.aiTitle || d.request.workspaceName || "A2UI";
    return text.length > 24 ? `${text.slice(0, 24)}…` : text;
  }
  if (d.kind === "permission-prompt") {
    return d.request.toolName || "Permission";
  }
  const first = d.request.questions[0];
  if (first?.header) return first.header;
  const text = first?.question ?? "Question";
  return text.length > 24 ? `${text.slice(0, 24)}…` : text;
}

// ── Main panel ───────────────────────────────────────────────────────────

export function DecisionPanel({
  compact = false,
  float = false,
  onInlineDetailChange,
}: {
  compact?: boolean;
  /** Standalone decision-float window mode. Unlike the main-window overlay
   *  (which floats `position: fixed` over the app) or lite/`compact` (which
   *  fills a fixed-size window), the float window *is* the panel and sizes
   *  itself to the card's natural height. So the panel must flow in normal
   *  document layout — not `position: fixed`, no `max-height` cap — otherwise
   *  it contributes zero height to the wrapper the float window measures
   *  (a fixed-position element is out of flow), the window never grows, and
   *  only a sliver of the card shows. */
  float?: boolean;
  /** Fired when the inline SessionDetail column toggles. The standalone
   *  decision-float window uses this to widen itself when detail expands. */
  onInlineDetailChange?: (open: boolean) => void;
} = {}) {
  const { t } = useTranslation();
  const {
    decisions,
    activeDecisionId,
    setActiveDecision,
  } = useDecisionStore();
  const setLiteDecisionHistorySessionId = useUIStore(
    (s) => s.setLiteDecisionHistorySessionId,
  );
  const decisionPanelCollapsed = useUIStore((s) => s.decisionPanelCollapsed);
  const setDecisionPanelCollapsed = useUIStore(
    (s) => s.setDecisionPanelCollapsed,
  );
  const sessionsList = useSessionsStore((s) => s.sessions);
  const [historyOpen, setHistoryOpen] = useState(false);
  // Review docs the agent attached to a fleet__ask card — shown as tabs in the
  // side column (same slot as the inline SessionDetail). Open by default so the
  // user sees them the moment the card arrives; the two share the column, so
  // opening one closes the other.
  const [docsOpen, setDocsOpen] = useState(true);

  // Escape key: block the active guard / permission-prompt decision.
  const { respond } = useDecisionStore();
  const respondToPermissionPrompt = useDecisionStore((s) => s.respondToPermissionPrompt);
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !activeDecisionId) return;
      const active = decisions.find((d) => d.id === activeDecisionId);
      if (active?.kind === "guard") {
        respond(active.id, false);
      } else if (active?.kind === "permission-prompt") {
        respondToPermissionPrompt(active.id, false);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [activeDecisionId, decisions, respond, respondToPermissionPrompt]);

  // Hold Option/Alt: temporarily fade the panel + disable pointer events so the
  // user can peek at content underneath it. Releasing restores. The blur guard
  // covers the macOS case where Option triggers a window/menu switch and we
  // never see the keyup event.
  const [peeking, setPeeking] = useState(false);
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Alt" && !e.repeat) setPeeking(true);
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === "Alt") setPeeking(false);
    };
    const onBlur = () => setPeeking(false);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  const cardAreaRef = useRef<HTMLDivElement>(null);
  const [widthTier, setWidthTier] = useState(0);

  // Track the viewport width so the panel re-measures its available space when
  // the window is resized. Without this, `window.innerWidth` is only read on the
  // renders triggered by state changes (decision switch, history toggle, …), so
  // a panel opened while the window was narrow keeps its narrow width after the
  // user widens the window — leaving the two-column detail layout overflowing.
  const [viewportWidth, setViewportWidth] = useState(() =>
    typeof window !== "undefined" ? window.innerWidth : 1400,
  );
  useEffect(() => {
    if (typeof window === "undefined") return;
    const onResize = () => setViewportWidth(window.innerWidth);
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const active = decisions.length > 0
    ? (decisions.find((d) => d.id === activeDecisionId) ?? decisions[0])
    : null;

  // Guard / permission-prompt decisions force-expand the panel — too
  // important to let the user miss because they collapsed it earlier for an
  // elicitation (the headless agent is blocked until answered).
  useEffect(() => {
    if (
      decisionPanelCollapsed &&
      (active?.kind === "guard" || active?.kind === "permission-prompt")
    ) {
      setDecisionPanelCollapsed(false);
    }
  }, [decisionPanelCollapsed, active?.kind, setDecisionPanelCollapsed]);

  // Lite/compact never collapses — the lite window is already small. The
  // standalone float window never collapses either: it exists solely to show
  // the card, and the collapsed state is shared via localStorage with the
  // main window, so honoring it here would leave the float a bare pill.
  const effectiveCollapsed = decisionPanelCollapsed && !compact && !float;

  const hasPreview =
    (active?.kind === "elicitation" || active?.kind === "fleet-ask") &&
    active.request.questions.some((q) => {
      const opts = q.options ?? [];
      // ElicitationQuestion uses `multiSelect`, FleetAskQuestion shares
      // the same field name — single check works for both.
      const multi = (q as { multiSelect?: boolean }).multiSelect ?? false;
      return !multi && opts.some((o) => o.preview);
    });

  // Build responsive width tiers: if content overflows vertically, widen the
  // panel step-by-step so markdown/long questions reflow wider instead of
  // forcing a scrollbar. Upper bound 1400px or viewport minus gutter.
  const widthTiers = useMemo(() => {
    const base = hasPreview ? 820 : 460;
    const vpMax = Math.min(viewportWidth - 24, 1400);
    const candidates = [base, 640, 820, 1040, 1200, vpMax];
    const unique = Array.from(new Set(candidates.filter((w) => w >= base && w <= vpMax)));
    unique.sort((a, b) => a - b);
    return unique;
  }, [hasPreview, viewportWidth]);

  // Reset tier when active decision changes.
  useEffect(() => {
    setWidthTier(0);
  }, [active?.id]);

  // Always start with history collapsed when the active decision changes;
  // each decision deserves a fresh, focused panel state.
  useEffect(() => {
    setHistoryOpen(false);
  }, [active?.id]);

  // SessionInfo for the active decision's session, looked up live so status
  // updates (e.g. running → done) reach the inline detail column.
  const activeSessionId = active?.request.sessionId ?? null;
  const activeSessionInfo = useMemo(
    () => (activeSessionId ? sessionsList.find((s) => s.id === activeSessionId) ?? null : null),
    [sessionsList, activeSessionId],
  );

  // Review docs attached to the active fleet__ask card (empty for other kinds).
  const reviewDocs =
    active?.kind === "fleet-ask" ? active.request.reviewDocs ?? [] : [];
  const hasReviewDocs = reviewDocs.length > 0;

  // Reset the side column to "docs open" whenever a card with docs becomes
  // active; cards without docs leave it closed so history behaves as before.
  useEffect(() => {
    setDocsOpen(hasReviewDocs);
  }, [active?.id, hasReviewDocs]);

  // Inline detail column is normal-mode only; lite has its own chip flow.
  // SessionDetail in standalone mode owns its own state (no shared global
  // store), so two detail views can coexist on different sessions.
  const inlineDetailActive = !compact && historyOpen && !!activeSessionInfo;
  const docsColumnActive = !compact && docsOpen && hasReviewDocs;
  // The docs panel and the SessionDetail history share the one side column;
  // docs win when both would be open (they reset closed on card change).
  const sideColumnActive = docsColumnActive || inlineDetailActive;

  useEffect(() => {
    onInlineDetailChange?.(sideColumnActive);
  }, [sideColumnActive, onInlineDetailChange]);

  // Bump tier when the card area overflows vertically, until no overflow or
  // we hit the maximum tier.
  useEffect(() => {
    const el = cardAreaRef.current;
    if (!el) return;
    // Elicitation / fleet-ask cards clip `.card_area` (overflow:hidden) and scroll
    // their body inside an inner `.card_scroll` instead — so `.card_area` itself
    // can never overflow vertically. Measure the real scroller: prefer the inner
    // `.card_scroll` when present, else `.card_area`. Without this, the widen-on-
    // overflow trigger reads scrollH==clientH on `.card_area` and never fires, so
    // the panel stays pinned at base width and the body reflows narrow + scrolls.
    const scroller = () =>
      (el.querySelector(`.${styles.card_scroll}`) as HTMLElement | null) ?? el;
    const check = () => {
      const s = scroller();
      if (widthTier < widthTiers.length - 1 && s.scrollHeight > s.clientHeight + 2) {
        setWidthTier((t) => Math.min(t + 1, widthTiers.length - 1));
      }
    };
    const ro = new ResizeObserver(check);
    ro.observe(el);
    const content = el.firstElementChild;
    if (content) ro.observe(content);
    const inner = el.querySelector(`.${styles.card_scroll}`);
    if (inner) ro.observe(inner);
    check();
    return () => ro.disconnect();
  }, [widthTier, widthTiers, active?.id]);

  if (!active) return null;

  const currentWidth = widthTiers[Math.min(widthTier, widthTiers.length - 1)];

  if (effectiveCollapsed) {
    return (
      <button
        type="button"
        className={`${styles.panel_collapsed_bar} ${active.kind === "guard" || active.kind === "permission-prompt" ? styles.panel_guard : active.kind === "plan-approval" ? styles.panel_plan : styles.panel_elicitation} ${peeking ? styles.panel_peeking : ""}`}
        onClick={() => setDecisionPanelCollapsed(false)}
        title={t("decision_panel.expand", "Expand panel")}
        aria-label={t("decision_panel.expand", "Expand panel")}
      >
        <svg className={styles.collapsed_bar_icon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          {active.kind === "guard" ? (
            <>
              <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
              <line x1="12" y1="9" x2="12" y2="13" />
              <line x1="12" y1="17" x2="12.01" y2="17" />
            </>
          ) : (
            <>
              <circle cx="12" cy="12" r="10" />
              <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
              <line x1="12" y1="17" x2="12.01" y2="17" />
            </>
          )}
        </svg>
        <span className={styles.collapsed_bar_label}>
          {t("decision_panel.collapsed_label", "Decision panel · {{count}} pending", {
            count: decisions.length,
          })}
        </span>
        <span className={styles.collapsed_bar_chevron} aria-hidden>▴</span>
      </button>
    );
  }

  // Detail column adds a fixed slab to the panel's overall width; clamp the
  // combined size to the viewport so the panel never escapes the screen.
  const DETAIL_COLUMN_WIDTH = 640;
  const vpClamp = viewportWidth - 24;
  // When the window is too narrow to seat the card and the detail column side by
  // side without dropping below their min-widths (detail 420 + card 380 = 800),
  // the two flex columns overflow the clamped panel and the card gets shoved
  // off-screen. Below this threshold, stack them vertically instead (card on
  // top, detail below) so everything stays inside the panel. The float window
  // manages its own width (it widens to ~920px for the side-by-side layout), so
  // it never needs stacking.
  const STACK_DETAIL_BELOW = 900;
  const stackDetail = sideColumnActive && !float && vpClamp < STACK_DETAIL_BELOW;
  const targetTotalWidth = sideColumnActive && !stackDetail
    ? DETAIL_COLUMN_WIDTH + currentWidth
    : currentWidth;
  const panelWidth = Math.min(targetTotalWidth, vpClamp);

  return (
    <div
      className={`${styles.panel} ${active.kind === "guard" || active.kind === "permission-prompt" ? styles.panel_guard : active.kind === "plan-approval" ? styles.panel_plan : styles.panel_elicitation} ${hasPreview ? styles.panel_wide : ""} ${compact ? styles.panel_compact : ""} ${float ? styles.panel_float : ""} ${peeking ? styles.panel_peeking : ""} ${sideColumnActive ? (stackDetail ? styles.panel_with_detail_stacked : styles.panel_with_detail) : ""}`}
      style={compact || float ? undefined : { width: `${panelWidth}px` }}
    >
      {sideColumnActive && (
        <div className={styles.detail_column}>
          {docsColumnActive ? (
            <ReviewDocsColumn
              docs={reviewDocs}
              sessionId={active.request.sessionId}
            />
          ) : (
            <SessionDetail inline sessionInfo={activeSessionInfo} />
          )}
        </div>
      )}
      <div className={styles.main_column}>
        {/* Panel toolbar: aligns the panel-level controls (collapse + history)
         *  into one slim top strip instead of two orphaned, absolutely-positioned
         *  buttons floating in the left gutter. Covers all card kinds (guard /
         *  plan / elicitation / fleet-ask) plus the standalone float window.
         *  Lite/`compact` keeps its own labeled history_jump chip below. */}
        {!compact && (
          <div className={styles.panel_toolbar}>
            <button
              type="button"
              className={styles.collapse_btn}
              onClick={() => setDecisionPanelCollapsed(true)}
              title={t("decision_panel.collapse", "Collapse panel")}
              aria-label={t("decision_panel.collapse", "Collapse panel")}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
                <polyline points="6 9 12 15 18 9" />
              </svg>
            </button>
            <span className={styles.panel_toolbar_spacer} />
            {/* Review-docs toggle: the agent attached `.md` / wiki docs to this
             *  card. Opening docs closes history (they share the side column). */}
            {hasReviewDocs && (
              <button
                type="button"
                className={`${styles.review_docs_toggle} ${docsColumnActive ? styles.review_docs_toggle_active : ""}`}
                onClick={() =>
                  setDocsOpen((v) => {
                    const next = !v;
                    if (next) setHistoryOpen(false);
                    return next;
                  })
                }
                title={t("review_docs.toggle", "Review documents")}
                aria-pressed={docsColumnActive}
              >
                {t("review_docs.chip", "📄 Docs · {{count}}", {
                  count: reviewDocs.length,
                })}
              </button>
            )}
            {/* History toggle opens the inline SessionDetail column. Neutral
             *  chip styling (no red alarm badge) since it's context, not alert. */}
            {active.request.sessionId && (
              <PastHistoryStrip
                key={active.id}
                sessionId={active.request.sessionId}
                expanded={historyOpen}
                onToggle={() =>
                  setHistoryOpen((v) => {
                    const next = !v;
                    if (next) setDocsOpen(false);
                    return next;
                  })
                }
              />
            )}
          </div>
        )}
        {/* Lite mode: a single chip-button swaps the lite body for a dedicated
         *  decision-history view (LiteDecisionHistory). Avoids stuffing the list
         *  into the narrow lite window. */}
        {active.request.sessionId && compact && (
          <button
            type="button"
            className={styles.history_jump}
            onClick={() => {
              const sid = active.request.sessionId;
              if (!sid) return;
              setLiteDecisionHistorySessionId(sid);
            }}
          >
            <span className={styles.history_jump_chevron}>↗</span>
            <span className={styles.history_jump_label}>
              {t(
                "decision_panel.view_session_history",
                "View this session's history",
              )}
            </span>
          </button>
        )}

        {/* Card area — scrollable, shows the active decision. Elicitation /
            fleet-ask cards own their internal scroll + flex-none footer, so the
            area switches to a clip-flex container for them (see .card_area_flex). */}
        <div
          className={`${styles.card_area} ${
            active.kind === "elicitation" || active.kind === "fleet-ask"
              ? styles.card_area_flex
              : ""
          }`}
          ref={cardAreaRef}
        >
          <DecisionCard key={active.id} decision={active} compact={compact} />
        </div>

        {/* Tab bar — always at the bottom */}
        <div className={styles.tab_bar}>
        {decisions.map((d) => (
          <button
            key={d.id}
            className={`${styles.tab} ${d.id === active.id ? styles.tab_active : ""} ${d.kind === "guard" || d.kind === "permission-prompt" ? styles.tab_guard : d.kind === "plan-approval" ? styles.tab_plan : styles.tab_elicitation}`}
            onClick={() => setActiveDecision(d.id)}
          >
            {d.kind === "guard" ? (
              <svg className={styles.tab_icon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                <line x1="12" y1="9" x2="12" y2="13" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
              </svg>
            ) : d.kind === "permission-prompt" ? (
              <svg className={styles.tab_icon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
                <path d="M7 11V7a5 5 0 0 1 10 0v4" />
              </svg>
            ) : d.kind === "plan-approval" ? (
              <svg className={styles.tab_icon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M9 11l3 3L22 4" />
                <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
              </svg>
            ) : (
              <svg className={styles.tab_icon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="12" cy="12" r="10" />
                <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
              </svg>
            )}
            <span className={styles.tab_label}>{tabLabel(d)}</span>
            {d.request.workspaceName && (
              <span className={styles.tab_workspace}>{d.request.workspaceName}</span>
            )}
            {d.request.sessionId && (
              <span className={styles.tab_session}>{shortId(d.request.sessionId)}</span>
            )}
          </button>
        ))}
        </div>
      </div>
    </div>
  );
}
