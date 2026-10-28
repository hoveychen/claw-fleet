import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { useDecisionStore } from "../store";
import { safeRemarkPlugins, safeRehypePlugins } from "../markdown/safeLinks";
import { normalizeSvgBlankLines, markdownUrlTransform } from "../markdown/plugins";
import { usePathMarkdown } from "../hooks/usePathLinks";
import { useDocumentTheme } from "../hooks/useDocumentTheme";
import {
  DecisionExplainAnswers,
  FlipConditionsButton,
  SteelmanButton,
  useCardExplain,
  type DecisionExplain,
} from "./DecisionExplainMarks";
import explainStyles from "./DecisionExplainMarks.module.css";
import { SelectionToolbar } from "./SelectionToolbar";
import { framePreviewSrcDoc } from "../decisionFrame";
import type {
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
import { StructuredCommandView } from "./StructuredCommandView";
import { useAutoFlip } from "./useAutoFlip";
import { AutoHeightFrame } from "./AutoHeightFrame";
import { DecisionAssetFrame } from "./DecisionAssetFrame";
import styles from "./DecisionPanel.module.css";
import { Presence } from "./Presence";
import { SkeletonText } from "./loading";
import { fromSubtask, isRecommendedOption, stripRecommendedSuffix } from "./dockKeys";



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
        <div className={styles.analysis}>
          {decision.analyzing
            ? <SkeletonText lines={3} />
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
            <Presence when={Boolean(menuOpen)}>{menuOpen && (
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
            )}</Presence>
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
        className={`${styles.collapse_chevron} ${!collapsed ? styles.collapse_chevron_open : ""}`}
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

/** The row right above a card's options: the collapse chevron and the
 *  single/multi toggle. The toggle sits here, not in the card header, so it is
 *  next to what it changes even when the question body runs long. */
function OptionsTools({
  collapse,
  canToggleMode,
  effectiveMulti,
  onToggleMode,
}: {
  collapse: { collapsed: boolean; onToggle: () => void; count: number; summary: string | null } | null;
  canToggleMode: boolean;
  effectiveMulti: boolean;
  onToggleMode: () => void;
}) {
  const { t } = useTranslation();
  if (!collapse && !canToggleMode) return null;
  return (
    <div className={styles.options_tools}>
      {collapse ? <OptionsCollapseBar {...collapse} /> : <span className={styles.options_tools_spacer} />}
      {canToggleMode && (
        <button
          type="button"
          className={`${styles.mode_toggle} ${effectiveMulti ? styles.mode_toggle_multi : ""}`}
          onClick={onToggleMode}
          title={t("fleet_ask.mode_tooltip", "Switch between single/multi select")}
        >
          {effectiveMulti
            ? t("fleet_ask.mode_multi", "Multi")
            : t("fleet_ask.mode_single", "Single")}
        </button>
      )}
    </div>
  );
}

function ElicitationCard({
  decision,
  compact = false,
  docked = false,
  natural = false,
}: {
  decision: ElicitationDecision;
  compact?: boolean;
  docked?: boolean;
  /** Natural height inside a scroller: no inner scroll, no footer cap. */
  natural?: boolean;
}) {
  const parked = decision.request.parked === true;
  const { t } = useTranslation();
  const mdComponents = usePathMarkdown(decision.request.sessionId);
  // Side questions from inside the question — a drag, or a click on one of the
  // agent's `[?text]` marks, brings up the same bar the transcript has. In the
  // panel the answers go to its side column; inline (SessionDetail's compact
  // card) they stay under the question (see DecisionExplainMarks).
  const { explain: explainMarks, inline: explainInline } = useCardExplain(decision.request.sessionId, decision.request.timestamp);
  const questionBodyRef = useRef<HTMLDivElement>(null);
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
  // Only allow override when question was originally single-select.
  const canToggleMode = !q.multiSelect && q.options.length > 0;

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
  // Terminal button, on the turn-completion card only: that card is raised
  // because the session finished its turn and is waiting for input, so ending
  // the task is a verdict the boss can actually make. A mid-turn
  // AskUserQuestion card has an agent blocked on an answer and keeps its
  // original Submit / Decline pair.
  //
  // These cards carry no `taskComplete` claim the way fleet__ask does, so the
  // button always closes the task as complete.
  //
  // A card raised by a sub-task never offers it: ending the task belongs to the
  // task's main conversation, not to one of the sessions it spawned.
  const canTerminate = request.turnCompletion === true && !fromSubtask(request);
  const handleTerminate = useCallback(
    () => declineElicitation(decision.id, "completed"),
    [declineElicitation, decision.id],
  );

  return (
    <div className={`${styles.card} ${styles.card_flex} ${natural ? styles.card_natural : ""}`}>
      <div className={styles.card_scroll}>
      {!docked && (
      <>
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
        {request.workspaceName && (
          <span className={styles.card_workspace}>{request.workspaceName}</span>
        )}
      </div>

      {request.aiTitle && (
        <div className={styles.card_subtitle}>{request.aiTitle}</div>
      )}
      </>
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
        {/* Stamped like a transcript row so `readAssistantSelection` accepts a
            selection here; the index is the question's, the anchor sent is empty. */}
        <div
          ref={questionBodyRef}
          className={`${styles.elicitation_question_text} ${explainStyles.body}`}
          data-role="assistant"
          data-msg-idx={step}
          data-explain-scope=""
        >
          <SelectionToolbar
            pane={questionBodyRef}
            scroller={questionBodyRef}
            enabled={explainMarks.enabled}
            busy={explainMarks.busy}
            onAsk={explainMarks.ask}
          />
          {q.header && (
            <span className={styles.elicitation_header}>{q.header}</span>
          )}
          <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>{normalizeSvgBlankLines(q.question)}</ReactMarkdown>
        </div>
        {explainMarks.enabled && <FlipConditionsButton explain={explainMarks} question={q.question} msgIdx={step} />}
        {explainInline && (
          <DecisionExplainAnswers answers={explainMarks.answers} loading={explainMarks.loading} onDismiss={explainMarks.dismiss} onFollowUp={explainMarks.followUp} />
        )}
      </div>
      </div>

      {/* Always-visible footer — options / "Other" / actions stay reachable
          without scrolling (flex-none tail; the body scrolls in .card_scroll). */}
      <div className={styles.card_footer}>
        {/* Manual collapse is offered whenever there is more than one option to
            fold (default stays expanded); only a single-option card stays open. */}
        <OptionsTools
          collapse={
            q.options.length > 1
              ? {
                  collapsed: optionsCollapsed,
                  onToggle: () => setOptionsCollapsed((v) => !v),
                  count: q.options.length,
                  summary: collapsedSummary,
                }
              : null
          }
          canToggleMode={canToggleMode}
          effectiveMulti={effectiveMulti}
          onToggleMode={() =>
            setElicitationMultiSelectOverride(decision.id, q.question, !effectiveMulti)
          }
        />
        {(q.options.length <= 1 || !optionsCollapsed) && (
        <SharedOptionsBlock
          decisionId={decision.id}
          sessionId={decision.request.sessionId}
          question={q}
          steelman={explainMarks.enabled ? { explain: explainMarks, question: q.question, msgIdx: step } : undefined}
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
        {canTerminate && (
          <button
            className={`${styles.btn} ${styles.btn_finish}`}
            onClick={handleTerminate}
            title={t(
              "elicitation.finish_task_tooltip",
              "Ending here marks this session complete.",
            )}
          >
            {t("elicitation.finish_task", "Finish task")}
          </button>
        )}
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
            className={`${styles.btn} ${styles.btn_primary}`}
            onClick={handleSubmit}
            disabled={!allAnswered}
          >
            {parked ? t("parked.submit", "Answer & wake the session") : t("elicitation.submit", "Submit")}
          </button>
        ) : (
          <button
            className={`${styles.btn} ${styles.btn_primary}`}
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
  steelman,
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
  /** What a per-option steelman ask needs; absent (the card's session cannot
   *  be forked) hides the button. */
  steelman?: { explain: DecisionExplain; question: string; msgIdx: number };
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

  // Only a click fills an option. A pre-filled "primary" button read as already
  // picked while Submit stayed disabled, so the recommendation is a tag instead.
  const list = (
    <div className={styles.elicitation_options}>
      {question.options.map((opt, index) => {
        const isSelected = selected.includes(opt.label);
        const recommended = isRecommendedOption(opt.label);
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
              aria-pressed={isSelected}
              onClick={() =>
                onToggle(decisionId, question.question, opt.label, effectiveMulti)
              }
              onMouseEnter={hasPreview ? () => setFocusedLabel(opt.label) : undefined}
              onFocus={hasPreview ? () => setFocusedLabel(opt.label) : undefined}
            >
              {/* The digit the dock's keyboard handler maps to this option; a
                  check once it is picked. */}
              {(index < 9 || isSelected) && (
                <span className={styles.elicitation_option_num} aria-hidden>
                  {isSelected ? "✓" : index + 1}
                </span>
              )}
              <span className={styles.elicitation_option_text}>
                <span className={styles.elicitation_option_label}>
                  {recommended ? stripRecommendedSuffix(opt.label) : opt.label}
                  {recommended && (
                    <span className={styles.elicitation_option_rec}>
                      {t("elicitation.recommended", "Recommended")}
                    </span>
                  )}
                </span>
                {opt.description && (
                  <span className={styles.elicitation_option_desc}>{opt.description}</span>
                )}
              </span>
            </button>
            {steelman && <SteelmanButton {...steelman} option={opt} />}
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

      {/* Free-text answer, always open: the reader can skip the options and
          just say what they want. ⌘↵ submits from here (see DecisionDock). */}
      <div className={styles.elicitation_other_block}>
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
          placeholder={t("decision.say_directly", "Or just say it…  ⌘↵ to send")}
          wikiMentions
        />
      </div>
    </div>
  );

  if (!hasPreview) return list;
  // Every option's preview is rendered into the same grid cell and only the
  // focused one is visible, so the box is always as tall as the tallest
  // preview. Sizing it to the focused preview alone made hover a feedback
  // loop: a different height shifted the options under a still pointer, which
  // then entered a neighbour, which resized the box again — endless flicker.
  const preview = (
    <div className={styles.elicitation_preview}>
      {question.options.map((opt) =>
        opt.preview ? (
          <div
            key={opt.label}
            className={`${styles.elicitation_preview_layer} ${opt.label === focusedLabel ? "" : styles.elicitation_preview_layer_hidden}`}
            aria-hidden={opt.label !== focusedLabel}
          >
            <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>{normalizeSvgBlankLines(opt.preview)}</ReactMarkdown>
          </div>
        ) : null,
      )}
    </div>
  );
  // Compact hosts — the inline card inside SessionDetail — stack the preview
  // under the options instead of splitting the narrow column in half. This used
  // to open an always-on-top Tauri subwindow; stacking keeps the preview inside
  // the card, which is where the option it belongs to is. The focused option's
  // label is captioned because the side-by-side layout's spatial pairing is
  // what stacking gives up.
  if (compact) {
    return (
      <div className={styles.elicitation_options_stacked_preview}>
        {list}
        <div className={styles.elicitation_preview_caption}>
          {focusedLabel || t("elicitation.preview", "Preview")}
        </div>
        {preview}
      </div>
    );
  }
  return (
    <div className={styles.elicitation_options_with_preview}>
      {list}
      {preview}
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
//
// The frame is deliberately NOT opaque. Agents author these previews against
// the card they can see and routinely style a `background: transparent` body
// with a light foreground; the frame used to paint itself `#fff`, which turned
// exactly that into light-grey-on-white — unreadable in the dark theme. Leaving
// it transparent lets the card's own themed surface show through, and
// `framePreviewSrcDoc` carries a matching `color-scheme` + default foreground
// into the document for previews that style nothing at all.
function fleetAskFrameStyle(theme: "dark" | "light") {
  return {
    width: "100%",
    border: "1px solid var(--decision-card-border, #ccc)",
    borderRadius: "0.4rem",
    background: "transparent",
    // Also propagates to the embedded document's UA defaults (form controls,
    // scrollbars) on the served-index.html path, which cannot take a prelude.
    colorScheme: theme,
  } as const;
}


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
  docked = false,
  natural = false,
}: {
  decision: FleetAskDecision;
  compact?: boolean;
  /** Pinned in DecisionDock: the dock's header line carries the label, step and
   *  single/multi switch, and the page already shows project and task title. */
  docked?: boolean;
  /** Natural height inside a scroller: no inner scroll, no footer cap. */
  natural?: boolean;
}) {
  const parked = decision.request.parked === true;
  const { t } = useTranslation();
  const mdComponents = usePathMarkdown(decision.request.sessionId);
  // Side questions from inside the question — a drag, or a click on one of the
  // agent's `[?text]` marks, brings up the same bar the transcript has. In the
  // panel the answers go to its side column; inline (SessionDetail's compact
  // card) they stay under the question (see DecisionExplainMarks).
  const { explain: explainMarks, inline: explainInline } = useCardExplain(decision.request.sessionId, decision.request.timestamp);
  const questionBodyRef = useRef<HTMLDivElement>(null);
  // The preview iframe is cross-origin, so the theme has to travel into it as a
  // value rather than through CSS custom properties.
  const theme = useDocumentTheme();
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
  // v3 terminal button. It is always present—the agent no longer hand-rolls a
  // "Task Done" option—and `taskComplete` only decides which verdict it carries:
  // the agent thinks the work is done → "End Task" (session closes as a success),
  // otherwise → "Abandon Task" (closes as unfinished). Either way the card
  // resolves as `cancelled` for the agent, which is what tells it to stop.
  const taskComplete = decision.request.taskComplete === true;
  const handleTerminate = useCallback(
    () => cancelFleetAsk(decision.id, taskComplete ? "completed" : "abandoned"),
    [cancelFleetAsk, decision.id, taskComplete],
  );

  return (
    <div className={`${styles.card} ${styles.card_flex} ${natural ? styles.card_natural : ""}`}>
      <div className={styles.card_scroll}>
      {!docked && (
      <>
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
        {request.workspaceName && (
          <span className={styles.card_workspace}>{request.workspaceName}</span>
        )}
      </div>

      {request.aiTitle && (
        <div className={styles.card_subtitle}>{request.aiTitle}</div>
      )}
      </>
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
        {/* Stamped like a transcript row so `readAssistantSelection` accepts a
            selection here; the index is the question's, the anchor sent is empty. */}
        <div
          ref={questionBodyRef}
          className={`${styles.elicitation_question_text} ${explainStyles.body}`}
          data-role="assistant"
          data-msg-idx={step}
          data-explain-scope=""
        >
          <SelectionToolbar
            pane={questionBodyRef}
            scroller={questionBodyRef}
            enabled={explainMarks.enabled}
            busy={explainMarks.busy}
            onAsk={explainMarks.ask}
          />
          {q.header && (
            <span className={styles.elicitation_header}>{q.header}</span>
          )}
          <ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={mdComponents}>{normalizeSvgBlankLines(q.question)}</ReactMarkdown>
        </div>
        {explainMarks.enabled && <FlipConditionsButton explain={explainMarks} question={q.question} msgIdx={step} />}
        {explainInline && (
          <DecisionExplainAnswers answers={explainMarks.answers} loading={explainMarks.loading} onDismiss={explainMarks.dismiss} onFollowUp={explainMarks.followUp} />
        )}

        {q.images && q.images.length > 0 ? (
          // Image-bearing card: load the served index.html (agent html or auto
          // gallery) so <img src="name"> resolves to the copied files — no
          // base64 in the tool call. `DecisionAssetFrame` picks how to reach
          // those bytes per host (custom protocol vs. parent-fetched srcDoc).
          <DecisionAssetFrame
            title={`fleet-ask-html-${decision.id}-${step}`}
            id={decision.id}
            qidx={`q${step}`}
            theme={theme}
            minHeight={200}
            style={fleetAskFrameStyle(theme)}
          />
        ) : (
          q.html && (
            <AutoHeightFrame
              title={`fleet-ask-html-${decision.id}-${step}`}
              srcDoc={framePreviewSrcDoc(q.html, theme)}
              minHeight={200}
              style={fleetAskFrameStyle(theme)}
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
        <OptionsTools
          collapse={
            opts.length + formFields.length > 1
              ? {
                  collapsed: optionsCollapsed,
                  onToggle: () => setOptionsCollapsed((v) => !v),
                  count: opts.length + formFields.length,
                  summary: collapsedSummary,
                }
              : null
          }
          canToggleMode={canToggleMode}
          effectiveMulti={effectiveMulti}
          onToggleMode={() =>
            setFleetAskMultiSelectOverride(decision.id, q.question, !effectiveMulti)
          }
        />
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
            steelman={explainMarks.enabled ? { explain: explainMarks, question: q.question, msgIdx: step } : undefined}
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
        {/* Sub-task cards have no terminal button: only the task's main
            conversation can end the task. */}
        {!fromSubtask(request) && (
        <button
          className={`${styles.btn} ${taskComplete ? styles.btn_finish : styles.btn_abandon}`}
          onClick={handleTerminate}
          title={
            taskComplete
              ? t("fleet_ask.finish_task_tooltip", "Ending here marks this session complete.")
              : t("fleet_ask.abandon_task_tooltip", "Ending here marks this session unfinished.")
          }
        >
          {taskComplete
            ? t("fleet_ask.finish_task", "Finish task")
            : t("fleet_ask.abandon_task", "Abandon task")}
        </button>
        )}
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
            className={`${styles.btn} ${styles.btn_primary}`}
            onClick={handleSubmit}
            disabled={!allAnswered}
          >
            {parked ? t("parked.submit", "Answer & wake the session") : t("fleet_ask.submit", "Submit")}
          </button>
        ) : (
          <button
            className={`${styles.btn} ${styles.btn_primary}`}
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

export function DecisionCard({
  decision,
  compact = false,
  docked = false,
  natural = false,
}: {
  decision: PendingDecision;
  compact?: boolean;
  docked?: boolean;
  natural?: boolean;
}) {
  switch (decision.kind) {
    case "guard":
      return <GuardCard decision={decision} />;
    case "elicitation":
      return <ElicitationCard decision={decision} compact={compact} docked={docked} natural={natural} />;
    case "fleet-ask":
      // Review docs are listed in the session's material rail, not here.
      return <FleetAskCard decision={decision} compact={compact} docked={docked} natural={natural} />;
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
