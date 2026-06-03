import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { invoke } from "@tauri-apps/api/core";
import {
  resolveTheme,
  useDecisionStore,
  useDetailStore,
  useProjectsStore,
  useSessionsStore,
  useTasksStore,
  useUIStore,
} from "../store";
import { safeMarkdownComponents, safeRemarkPlugins } from "../markdown/safeLinks";
import type {
  DecisionHistoryRecord,
  ElicitationAttachment,
  ElicitationDecision,
  FleetAskDecision,
  FleetAskFormField,
  GuardDecision,
  PendingDecision,
  PlanApprovalDecision,
  SessionPendingDecision,
} from "../types";
import { A2uiRenderCard } from "./A2uiRenderCard";
import { ChatComposer, type ChatComposerHandle } from "./ChatComposer";
import { SessionDetail } from "./SessionDetail";
import { StructuredCommandView } from "./StructuredCommandView";
import styles from "./DecisionPanel.module.css";

function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

// ── Master / Worker context chip ───────────────────────────────────────────
//
// When the elicitation / guard request came from a Master or Worker session
// (per `session_kind` on the persisted FleetSession), render a small chip
// alongside the workspaceName so the user knows *which task's master* is
// asking. PRD §5.x / TASKS P14.

function useTaskContextForSession(sessionId: string | null | undefined) {
  const fleetSessions = useProjectsStore((s) => s.fleetSessions);
  const tasks = useTasksStore((s) => s.tasks);
  return useMemo(() => {
    if (!sessionId) return null;
    const fs = fleetSessions.find((s) => s.id === sessionId);
    if (!fs || !fs.sessionKind || fs.sessionKind === "regular") return null;
    const task = fs.taskId ? tasks.find((t) => t.id === fs.taskId) : undefined;
    return {
      kind: fs.sessionKind,
      taskTitle: task?.title ?? null,
      pItemId: fs.pItemId ?? null,
    };
  }, [fleetSessions, sessionId, tasks]);
}

function TaskMasterChip({ sessionId }: { sessionId: string | null | undefined }) {
  const { t } = useTranslation();
  const ctx = useTaskContextForSession(sessionId);
  if (!ctx) return null;
  const role = ctx.kind === "master" ? t("master.chip_master", "Master") : t("master.chip_worker", "Worker");
  const titleLabel = ctx.taskTitle
    ? `Task ${ctx.taskTitle} · ${role}`
    : `Task · ${role}`;
  const fullLabel = ctx.kind === "worker" && ctx.pItemId
    ? `${titleLabel} · ${ctx.pItemId}`
    : titleLabel;
  return (
    <span className={styles.master_chip} title={fullLabel}>
      {fullLabel}
    </span>
  );
}

// ── Guard card renderer ────────────────────────────────────────────────────

/**
 * A token is a CLI flag (e.g. `-s=clw`, `--verbose`) — exclude it from the
 * remembered prefix so `patchwright-cli -s=clw eval ...` becomes
 * `patchwright-cli eval` and not `patchwright-cli -s=clw`.
 */
function isFlagToken(tok: string): boolean {
  return tok.length > 1 && tok.startsWith("-");
}

/**
 * Prefix for a single AST leaf: `argv[0]` plus the first non-flag token after
 * it (the "subcommand" in the `git push` / `npm test` / `patchwright-cli eval`
 * sense).  Falls back to just `argv[0]` if every later token is a flag.
 */
function computeLeafAllowPrefix(argv: string[]): string {
  const head = argv[0];
  if (!head) return "";
  const sub = argv.slice(1).find((t) => !isFlagToken(t));
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

function GuardCard({ decision }: { decision: GuardDecision }) {
  const { t } = useTranslation();
  const { respond } = useDecisionStore();
  const req = decision.request;

  const allowPrefixes = useMemo(() => computeGuardAllowPrefixes(req), [req]);
  const sourceTag = req.riskTags[0] ?? null;
  const [menuOpen, setMenuOpen] = useState(false);
  const [blockReason, setBlockReason] = useState("");
  const menuWrapRef = useRef<HTMLDivElement | null>(null);

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
        <TaskMasterChip sessionId={req.sessionId ?? null} />
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
            : <ReactMarkdown remarkPlugins={safeRemarkPlugins} components={safeMarkdownComponents}>{decision.analysis ?? ""}</ReactMarkdown>}
        </div>
      )}

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
              <div className={styles.always_allow_menu_panel} role="menu">
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
        <button className={`${styles.btn} ${styles.btn_block}`} onClick={handleBlock}>
          {t("guard.block", "Block")}
        </button>
      </div>
    </div>
  );
}

// ── Elicitation card renderer (multi-step wizard) ─────────────────────────

function ElicitationCard({ decision, compact = false }: { decision: ElicitationDecision; compact?: boolean }) {
  const { t } = useTranslation();
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
    <div className={styles.card}>
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
        <TaskMasterChip sessionId={request.sessionId ?? null} />
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
          <ReactMarkdown remarkPlugins={safeRemarkPlugins} components={safeMarkdownComponents}>{q.question}</ReactMarkdown>
        </div>
      </div>

      {/* Pinned footer — options / "Other" / actions stay visible without scrolling. */}
      <div className={styles.card_footer}>
        <SharedOptionsBlock
          decisionId={decision.id}
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

        <div className={styles.actions}>
        <button
          className={`${styles.btn} ${styles.btn_secondary}`}
          onClick={handleDecline}
        >
          {t("elicitation.decline", "Decline")}
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
            {t("elicitation.submit", "Submit")}
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
  options: Array<{ label: string; description: string; preview?: string }>;
}

// Renders the option list + "Other" input. Splits into side-by-side layout
// when any option carries a preview (single-select only, per AskUserQuestion spec).
// Shared between ElicitationCard (v1) and FleetAskCard (v2) — both pass their
// own store actions in via callbacks, so the same UI surface drives both.
function SharedOptionsBlock({
  decisionId,
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
          <ReactMarkdown remarkPlugins={safeRemarkPlugins} components={safeMarkdownComponents}>{focusedPreview}</ReactMarkdown>
        ) : null}
      </div>
    </div>
  );
}

// ── Plan-approval card renderer ─────────────────────────────────────────

function PlanApprovalCard({ decision }: { decision: PlanApprovalDecision }) {
  const { t } = useTranslation();
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
          <ReactMarkdown remarkPlugins={safeRemarkPlugins} components={safeMarkdownComponents}>
            {decision.editedPlan ?? req.planContent}
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

// ── Session-pending card (wait-for-input after agent end-of-turn) ────────

function SessionPendingCard({ decision }: { decision: SessionPendingDecision }) {
  const { t } = useTranslation();
  const {
    setSessionPendingFollowUp,
    markSessionPendingStatus,
    submitSessionPendingFollowUp,
  } = useDecisionStore();
  const sessionsList = useSessionsStore((s) => s.sessions);
  const openDetail = useDetailStore((s) => s.open);
  const req = decision.request;

  // Live SessionInfo, if available — gives us aiTitle and the latest
  // assistant message preview without re-fetching the transcript.
  const info = useMemo(
    () => sessionsList.find((s) => s.id === req.sessionId),
    [sessionsList, req.sessionId],
  );

  const lastMessage = info?.lastMessagePreview?.trim() || req.promptPreview;
  const titleText = info?.aiTitle || req.workspaceName || t("session_pending.title", "Session waiting for input");

  const handleStatusClick = useCallback(
    (statusId: string) => {
      if (decision.submitting) return;
      markSessionPendingStatus(decision.id, statusId);
    },
    [decision.id, decision.submitting, markSessionPendingStatus],
  );

  const handleFollowUpSubmit = useCallback(() => {
    if (decision.submitting || !decision.followUpText.trim()) return;
    submitSessionPendingFollowUp(decision.id);
  }, [decision.id, decision.followUpText, decision.submitting, submitSessionPendingFollowUp]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // IME composing 中的 Enter（中文/日文/韩文输入法确认候选词）放行给 IME。
      // keyCode === 229 是 Chromium 在 IME 处理期间的兜底信号。
      if (e.nativeEvent.isComposing || e.keyCode === 229) return;
      if (e.key !== "Enter") return;
      if (e.shiftKey) return;
      e.preventDefault();
      handleFollowUpSubmit();
    },
    [handleFollowUpSubmit],
  );

  const handleOpenDetail = useCallback(() => {
    if (info) {
      openDetail(info);
    }
  }, [info, openDetail]);

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
          <circle cx="12" cy="12" r="10" />
          <polyline points="12 6 12 12 16 14" />
        </svg>
        <span className={styles.card_title}>
          {t("session_pending.title", "Session waiting for input")}
        </span>
        {req.workspaceName && (
          <span className={styles.card_workspace}>{req.workspaceName}</span>
        )}
      </div>

      {info?.aiTitle && (
        <div className={styles.card_subtitle}>{titleText}</div>
      )}

      <div className={styles.analysis}>
        <ReactMarkdown remarkPlugins={safeRemarkPlugins} components={safeMarkdownComponents}>
          {lastMessage || ""}
        </ReactMarkdown>
      </div>

      <div className={styles.actions}>
        {req.terminalColumns.map((col) => (
          <button
            key={col.id}
            className={`${styles.btn} ${styles.btn_allow}`}
            onClick={() => handleStatusClick(col.id)}
            disabled={decision.submitting}
            title={t("session_pending.mark_status_tip", "Move this session into {{name}}", {
              name: col.name,
            })}
          >
            {col.name}
          </button>
        ))}
        {info && (
          <button
            type="button"
            className={styles.btn}
            onClick={handleOpenDetail}
            disabled={decision.submitting}
          >
            {t("session_pending.open_detail", "Open session detail")}
          </button>
        )}
      </div>

      <textarea
        className={styles.plan_feedback}
        value={decision.followUpText}
        onChange={(e) => setSessionPendingFollowUp(decision.id, e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={t(
          "session_pending.followup_placeholder",
          "Type a follow-up prompt (Enter to send, Shift+Enter for newline)…",
        )}
        rows={3}
        disabled={decision.submitting}
      />
      <div className={styles.actions}>
        <button
          type="button"
          className={`${styles.btn} ${styles.btn_allow}`}
          onClick={handleFollowUpSubmit}
          disabled={decision.submitting || !decision.followUpText.trim()}
        >
          {decision.submitting
            ? t("session_pending.sending", "Sending…")
            : t("session_pending.send", "Send follow-up")}
        </button>
      </div>
    </div>
  );
}

// ── fleet__ask card renderer (MCP-tool variant) ─────────────────────────
//
// Mirrors the elicitation card: stepped questions, header chip, workspace +
// AI title chips. Extends it with two optional render hooks per the P3 plan:
//   * `html` field non-empty → sandboxed `<iframe srcdoc>` between the
//     question body and the answer controls. `sandbox=""` means: no scripts,
//     no same-origin, no forms, no top-navigation, no popups. The agent gets
//     a static preview without an XSS vector.
//   * `formFields` non-empty → dynamic controls dispatched on `kind`
//     (text / textarea / number / select / radio / checkbox).
// `options` still renders the existing button grid. The three sections compose
// freely — a single question can carry any subset.

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

function FleetAskCard({
  decision,
  compact = false,
}: {
  decision: FleetAskDecision;
  compact?: boolean;
}) {
  const { t } = useTranslation();
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
    <div className={styles.card}>
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
          {t("fleet_ask.title", "Agent Question (fleet__ask)")}
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
        <TaskMasterChip sessionId={request.sessionId ?? null} />
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
          <ReactMarkdown remarkPlugins={safeRemarkPlugins} components={safeMarkdownComponents}>{q.question}</ReactMarkdown>
        </div>

        {q.html && (
          <iframe
            title={`fleet-ask-html-${decision.id}-${step}`}
            sandbox=""
            srcDoc={q.html}
            style={{
              width: "100%",
              minHeight: "200px",
              border: "1px solid var(--decision-card-border, #ccc)",
              borderRadius: "0.4rem",
              background: "#fff",
            }}
          />
        )}
      </div>

      {/* Pinned footer — form fields / options / "Other" / actions stay visible without scrolling. */}
      <div className={styles.card_footer}>
        {formFields.length > 0 && (
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

        {/* Render unconditionally so the free-text "Other" composer (which lives
            inside SharedOptionsBlock, outside its options .map) persists even when
            the agent supplied no options. A zero-option fleet__ask card would
            otherwise leave the user no way to type a free answer — v1's
            ElicitationCard renders this block unconditionally, so match it. With
            an empty options array SharedOptionsBlock renders just the Other input. */}
        {
          <SharedOptionsBlock
            decisionId={decision.id}
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
        }
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

        <div className={styles.actions}>
        <button
          className={`${styles.btn} ${styles.btn_secondary}`}
          onClick={handleCancel}
        >
          {t("fleet_ask.cancel", "Cancel")}
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
            {t("fleet_ask.submit", "Submit")}
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
    case "session-pending":
      return <SessionPendingCard decision={decision} />;
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

  if (records.length === 0) return null;

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
      <span className={styles.history_handle_count}>{records.length}</span>
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
  if (d.kind === "session-pending") {
    const text = d.request.promptPreview || d.request.workspaceName || "Pending";
    return text.length > 24 ? `${text.slice(0, 24)}…` : text;
  }
  if (d.kind === "a2ui-render") {
    const text = d.request.aiTitle || d.request.workspaceName || "A2UI";
    return text.length > 24 ? `${text.slice(0, 24)}…` : text;
  }
  const first = d.request.questions[0];
  if (first?.header) return first.header;
  const text = first?.question ?? "Question";
  return text.length > 24 ? `${text.slice(0, 24)}…` : text;
}

// ── Main panel ───────────────────────────────────────────────────────────

export function DecisionPanel({
  compact = false,
  onInlineDetailChange,
}: {
  compact?: boolean;
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

  // Escape key: block the active guard decision.
  const { respond } = useDecisionStore();
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !activeDecisionId) return;
      const active = decisions.find((d) => d.id === activeDecisionId);
      if (active?.kind === "guard") {
        respond(active.id, false);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [activeDecisionId, decisions, respond]);

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
  // Whether the scroll viewport still has content below the fold — drives the
  // bottom fade hint so users (especially on external mice, where macOS hides
  // the overlay scrollbar) get a visual cue that more options lie below.
  const [canScrollDown, setCanScrollDown] = useState(false);

  const active = decisions.length > 0
    ? (decisions.find((d) => d.id === activeDecisionId) ?? decisions[0])
    : null;

  // Guard decisions force-expand the panel — too important to let the user
  // miss because they collapsed it earlier for an elicitation.
  useEffect(() => {
    if (decisionPanelCollapsed && active?.kind === "guard") {
      setDecisionPanelCollapsed(false);
    }
  }, [decisionPanelCollapsed, active?.kind, setDecisionPanelCollapsed]);

  // Lite/compact never collapses — the lite window is already small.
  const effectiveCollapsed = decisionPanelCollapsed && !compact;

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
    const vpMax = typeof window !== "undefined"
      ? Math.min(window.innerWidth - 24, 1400)
      : 1400;
    const candidates = [base, 640, 820, 1040, 1200, vpMax];
    const unique = Array.from(new Set(candidates.filter((w) => w >= base && w <= vpMax)));
    unique.sort((a, b) => a - b);
    return unique;
  }, [hasPreview]);

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

  // Inline detail column is normal-mode only; lite has its own chip flow.
  // SessionDetail in standalone mode owns its own state (no shared store with
  // the KanbanView drawer), so the two can coexist on different sessions.
  const inlineDetailActive = !compact && historyOpen && !!activeSessionInfo;

  useEffect(() => {
    onInlineDetailChange?.(inlineDetailActive);
  }, [inlineDetailActive, onInlineDetailChange]);

  // Bump tier when the card area overflows vertically, until no overflow or
  // we hit the maximum tier.
  useEffect(() => {
    const el = cardAreaRef.current;
    if (!el) return;
    const check = () => {
      if (widthTier < widthTiers.length - 1 && el.scrollHeight > el.clientHeight + 2) {
        setWidthTier((t) => Math.min(t + 1, widthTiers.length - 1));
      }
      setCanScrollDown(el.scrollHeight - el.scrollTop - el.clientHeight > 2);
    };
    const ro = new ResizeObserver(check);
    ro.observe(el);
    const content = el.firstElementChild;
    if (content) ro.observe(content);
    check();
    return () => ro.disconnect();
  }, [widthTier, widthTiers, active?.id]);

  if (!active) return null;

  const currentWidth = widthTiers[Math.min(widthTier, widthTiers.length - 1)];

  if (effectiveCollapsed) {
    return (
      <button
        type="button"
        className={`${styles.panel_collapsed_bar} ${active.kind === "guard" ? styles.panel_guard : active.kind === "plan-approval" ? styles.panel_plan : styles.panel_elicitation} ${peeking ? styles.panel_peeking : ""}`}
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
  const targetTotalWidth = inlineDetailActive
    ? DETAIL_COLUMN_WIDTH + currentWidth
    : currentWidth;
  const vpClamp =
    typeof window !== "undefined" ? window.innerWidth - 24 : targetTotalWidth;
  const panelWidth = Math.min(targetTotalWidth, vpClamp);

  return (
    <div
      className={`${styles.panel} ${active.kind === "guard" ? styles.panel_guard : active.kind === "plan-approval" ? styles.panel_plan : styles.panel_elicitation} ${hasPreview ? styles.panel_wide : ""} ${compact ? styles.panel_compact : ""} ${peeking ? styles.panel_peeking : ""} ${inlineDetailActive ? styles.panel_with_detail : ""}`}
      style={compact ? undefined : { width: `${panelWidth}px` }}
    >
      {inlineDetailActive && (
        <div className={styles.detail_column}>
          <SessionDetail inline sessionInfo={activeSessionInfo} />
        </div>
      )}
      <div className={styles.main_column}>
        {!compact && (
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
        )}
        {/* Past-history context.
         *  - Normal mode: header toggles the inline SessionDetail column
         *    (managed by DecisionPanel state).
         *  - Lite mode: a single chip-button swaps the lite body for a
         *    dedicated decision-history view (LiteDecisionHistory). Avoids
         *    stuffing the list into the narrow lite window. */}
        {active.request.sessionId && !compact && (
          <PastHistoryStrip
            key={active.id}
            sessionId={active.request.sessionId}
            expanded={historyOpen}
            onToggle={() => setHistoryOpen((v) => !v)}
          />
        )}
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

        {/* Card area — scrollable, shows the active decision */}
        <div
          className={styles.card_area}
          ref={cardAreaRef}
          data-more={canScrollDown ? "true" : "false"}
          onScroll={(e) => {
            const el = e.currentTarget;
            setCanScrollDown(el.scrollHeight - el.scrollTop - el.clientHeight > 2);
          }}
        >
          <DecisionCard key={active.id} decision={active} compact={compact} />
          {/* Sticky bottom fade — hints that more options remain below the fold. */}
          <div className={styles.card_fade} aria-hidden="true" />
        </div>

        {/* Tab bar — always at the bottom */}
        <div className={styles.tab_bar}>
        {decisions.map((d) => (
          <button
            key={d.id}
            className={`${styles.tab} ${d.id === active.id ? styles.tab_active : ""} ${d.kind === "guard" ? styles.tab_guard : d.kind === "plan-approval" ? styles.tab_plan : styles.tab_elicitation}`}
            onClick={() => setActiveDecision(d.id)}
          >
            {d.kind === "guard" ? (
              <svg className={styles.tab_icon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                <line x1="12" y1="9" x2="12" y2="13" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
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
