import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useDecisionStore } from "../store";
import { safeMarkdownComponents } from "../markdown/safeLinks";
import type {
  ElicitationDecision,
  GuardDecision,
  PendingDecision,
  PlanApprovalDecision,
} from "../types";
import styles from "./DecisionPanel.module.css";

function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

// ── Guard card renderer ────────────────────────────────────────────────────

function GuardCard({ decision }: { decision: GuardDecision }) {
  const { t } = useTranslation();
  const { respond } = useDecisionStore();
  const req = decision.request;

  const handleAllow = useCallback(() => respond(decision.id, true), [respond, decision.id]);
  const handleBlock = useCallback(() => respond(decision.id, false), [respond, decision.id]);

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

      <div className={styles.command}>{req.command}</div>

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
            : <ReactMarkdown remarkPlugins={[remarkGfm]} components={safeMarkdownComponents}>{decision.analysis ?? ""}</ReactMarkdown>}
        </div>
      )}

      <div className={styles.actions}>
        <button className={`${styles.btn} ${styles.btn_allow}`} onClick={handleAllow}>
          {t("guard.allow", "Allow")}
        </button>
        <button className={`${styles.btn} ${styles.btn_block}`} onClick={handleBlock}>
          {t("guard.block", "Block")}
        </button>
      </div>
    </div>
  );
}

// ── Elicitation card renderer (multi-step wizard) ─────────────────────────

function ElicitationCard({ decision }: { decision: ElicitationDecision }) {
  const { t } = useTranslation();
  const {
    submitElicitation,
    declineElicitation,
    toggleElicitationOption,
    setElicitationCustomAnswer,
    setElicitationMultiSelectOverride,
    setElicitationStep,
  } = useDecisionStore();
  const otherInputRef = useRef<HTMLTextAreaElement>(null);

  const { step, request, selections, customAnswers, multiSelectOverrides } = decision;
  const total = request.questions.length;
  const q = request.questions[step];
  const isLast = step === total - 1;

  // Effective multi-select: the question's own flag OR a user-forced override.
  const effectiveMulti = q.multiSelect || multiSelectOverrides[q.question] === true;
  const canToggleMode = !q.multiSelect; // Only allow override when question was originally single-select.

  const selected = selections[q.question] || [];
  const customText = customAnswers[q.question] || "";
  const hasAnswer = selected.length > 0 || customText.trim().length > 0;

  const allAnswered = request.questions.every((qq) => {
    const sel = selections[qq.question] || [];
    const custom = customAnswers[qq.question]?.trim();
    return sel.length > 0 || (custom != null && custom.length > 0);
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
      </div>

      {request.aiTitle && (
        <div className={styles.card_subtitle}>{request.aiTitle}</div>
      )}

      {total > 1 && (
        <div className={styles.elicitation_dots}>
          {request.questions.map((qq, i) => {
            const answered =
              (selections[qq.question] || []).length > 0 ||
              (customAnswers[qq.question]?.trim().length ?? 0) > 0;
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
          <ReactMarkdown remarkPlugins={[remarkGfm]} components={safeMarkdownComponents}>{q.question}</ReactMarkdown>
        </div>
        <OptionsBlock
          decisionId={decision.id}
          question={q}
          effectiveMulti={effectiveMulti}
          selected={selected}
          onToggle={toggleElicitationOption}
          otherInputRef={otherInputRef}
          customText={customText}
          onCustomChange={(val) => setElicitationCustomAnswer(decision.id, q.question, val)}
        />
      </div>

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
  );
}

// Renders the option list + "Other" input. Splits into side-by-side layout
// when any option carries a preview (single-select only, per AskUserQuestion spec).
function OptionsBlock({
  decisionId,
  question,
  effectiveMulti,
  selected,
  onToggle,
  otherInputRef,
  customText,
  onCustomChange,
}: {
  decisionId: string;
  question: ElicitationDecision["request"]["questions"][number];
  effectiveMulti: boolean;
  selected: string[];
  onToggle: (id: string, questionText: string, label: string, multiSelect: boolean) => void;
  otherInputRef: React.RefObject<HTMLTextAreaElement | null>;
  customText: string;
  onCustomChange: (val: string) => void;
}) {
  const { t } = useTranslation();
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

  useEffect(() => {
    const el = otherInputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [customText, otherInputRef]);

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
          // Focus the Other textarea so the user can start editing immediately.
          requestAnimationFrame(() => {
            const el = otherInputRef.current;
            if (el) {
              el.focus();
              // Place caret at the end.
              el.setSelectionRange(el.value.length, el.value.length);
            }
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

      <div
        className={`${styles.elicitation_other} ${customText ? styles.elicitation_other_active : ""}`}
        onClick={() => otherInputRef.current?.focus()}
      >
        <span className={styles.elicitation_option_label}>
          {t("elicitation.other", "Other")}
        </span>
        <textarea
          ref={otherInputRef}
          className={styles.elicitation_other_input}
          rows={1}
          placeholder={t("elicitation.other_placeholder", "Type your answer…")}
          value={customText}
          onChange={(e) => onCustomChange(e.target.value)}
          onInput={(e) => {
            const el = e.currentTarget;
            el.style.height = "auto";
            el.style.height = `${el.scrollHeight}px`;
          }}
        />
      </div>
    </div>
  );

  if (!hasPreview) return list;
  return (
    <div className={styles.elicitation_options_with_preview}>
      {list}
      <div className={styles.elicitation_preview}>
        {focusedPreview ? (
          <ReactMarkdown remarkPlugins={[remarkGfm]} components={safeMarkdownComponents}>{focusedPreview}</ReactMarkdown>
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
          <ReactMarkdown remarkPlugins={[remarkGfm]} components={safeMarkdownComponents}>
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

// ── Card dispatcher ──────────────────────────────────────────────────────

function DecisionCard({ decision }: { decision: PendingDecision }) {
  switch (decision.kind) {
    case "guard":
      return <GuardCard decision={decision} />;
    case "elicitation":
      return <ElicitationCard decision={decision} />;
    case "plan-approval":
      return <PlanApprovalCard decision={decision} />;
    default:
      return null;
  }
}

// ── Tab label helper ─────────────────────────────────────────────────────

function tabLabel(d: PendingDecision): string {
  if (d.kind === "guard") {
    return d.request.toolName || "Guard";
  }
  if (d.kind === "plan-approval") {
    return d.request.aiTitle || "Plan";
  }
  const first = d.request.questions[0];
  if (first?.header) return first.header;
  const text = first?.question ?? "Question";
  return text.length > 24 ? `${text.slice(0, 24)}…` : text;
}

// ── Main panel ───────────────────────────────────────────────────────────

export function DecisionPanel({ compact = false }: { compact?: boolean } = {}) {
  const {
    decisions,
    activeDecisionId,
    setActiveDecision,
  } = useDecisionStore();

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

  const cardAreaRef = useRef<HTMLDivElement>(null);
  const [widthTier, setWidthTier] = useState(0);

  const active = decisions.length > 0
    ? (decisions.find((d) => d.id === activeDecisionId) ?? decisions[0])
    : null;

  const hasPreview =
    active?.kind === "elicitation" &&
    active.request.questions.some((q) => !q.multiSelect && q.options.some((o) => o.preview));

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

  // Bump tier when the card area overflows vertically, until no overflow or
  // we hit the maximum tier.
  useEffect(() => {
    const el = cardAreaRef.current;
    if (!el) return;
    const check = () => {
      if (widthTier < widthTiers.length - 1 && el.scrollHeight > el.clientHeight + 2) {
        setWidthTier((t) => Math.min(t + 1, widthTiers.length - 1));
      }
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

  return (
    <div
      className={`${styles.panel} ${active.kind === "guard" ? styles.panel_guard : active.kind === "plan-approval" ? styles.panel_plan : styles.panel_elicitation} ${hasPreview ? styles.panel_wide : ""} ${compact ? styles.panel_compact : ""}`}
      style={compact ? undefined : { width: `${currentWidth}px` }}
    >
      {/* Card area — scrollable, shows the active decision */}
      <div className={styles.card_area} ref={cardAreaRef}>
        <DecisionCard key={active.id} decision={active} />
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
  );
}
