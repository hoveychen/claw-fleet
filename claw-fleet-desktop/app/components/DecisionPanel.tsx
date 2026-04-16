import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useDecisionStore } from "../store";
import type {
  ElicitationDecision,
  ElicitationRequest,
  GuardDecision,
  GuardRequest,
  PendingDecision,
} from "../types";
import styles from "./DecisionPanel.module.css";

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
            : decision.analysis}
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
    setElicitationStep,
  } = useDecisionStore();
  const otherInputRef = useRef<HTMLInputElement>(null);

  const { step, request, selections, customAnswers } = decision;
  const total = request.questions.length;
  const q = request.questions[step];
  const isLast = step === total - 1;

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
        {request.workspaceName && (
          <span className={styles.card_workspace}>{request.workspaceName}</span>
        )}
      </div>

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
          {q.question}
        </div>
        <div className={styles.elicitation_options}>
          {q.options.map((opt) => {
            const isSelected = selected.includes(opt.label);
            return (
              <button
                key={opt.label}
                className={`${styles.elicitation_option} ${isSelected ? styles.elicitation_option_selected : ""}`}
                onClick={() =>
                  toggleElicitationOption(
                    decision.id,
                    q.question,
                    opt.label,
                    q.multiSelect,
                  )
                }
              >
                <span className={styles.elicitation_option_label}>
                  {opt.label}
                </span>
                {opt.description && (
                  <span className={styles.elicitation_option_desc}>
                    {opt.description}
                  </span>
                )}
              </button>
            );
          })}

          <div
            className={`${styles.elicitation_other} ${customText ? styles.elicitation_other_active : ""}`}
            onClick={() => otherInputRef.current?.focus()}
          >
            <span className={styles.elicitation_option_label}>
              {t("elicitation.other", "Other")}
            </span>
            <input
              ref={otherInputRef}
              className={styles.elicitation_other_input}
              type="text"
              placeholder={t("elicitation.other_placeholder", "Type your answer…")}
              value={customText}
              onChange={(e) =>
                setElicitationCustomAnswer(decision.id, q.question, e.target.value)
              }
            />
          </div>
        </div>
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

// ── Card dispatcher ──────────────────────────────────────────────────────

function DecisionCard({ decision }: { decision: PendingDecision }) {
  switch (decision.kind) {
    case "guard":
      return <GuardCard decision={decision} />;
    case "elicitation":
      return <ElicitationCard decision={decision} />;
    default:
      return null;
  }
}

// ── Tab label helper ─────────────────────────────────────────────────────

function tabLabel(d: PendingDecision): string {
  if (d.kind === "guard") {
    return d.request.toolName || "Guard";
  }
  const first = d.request.questions[0];
  if (first?.header) return first.header;
  const text = first?.question ?? "Question";
  return text.length > 24 ? `${text.slice(0, 24)}…` : text;
}

// ── Main panel ───────────────────────────────────────────────────────────

export function DecisionPanel() {
  const {
    decisions,
    activeDecisionId,
    setActiveDecision,
    addGuardRequest,
    addElicitationRequest,
  } = useDecisionStore();

  // Listen for guard-request events from the Rust backend.
  useEffect(() => {
    const unlisten = listen<GuardRequest>("guard-request", (e) => {
      addGuardRequest(e.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addGuardRequest]);

  // Listen for elicitation-request events from the Rust backend.
  useEffect(() => {
    const unlisten = listen<ElicitationRequest>("elicitation-request", (e) => {
      addElicitationRequest(e.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addElicitationRequest]);

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

  if (decisions.length === 0) return null;

  const active = decisions.find((d) => d.id === activeDecisionId) ?? decisions[0];

  return (
    <div className={styles.panel}>
      {/* Card area — scrollable, shows the active decision */}
      <div className={styles.card_area}>
        <DecisionCard key={active.id} decision={active} />
      </div>

      {/* Tab bar — always at the bottom */}
      <div className={styles.tab_bar}>
        {decisions.map((d) => (
          <button
            key={d.id}
            className={`${styles.tab} ${d.id === active.id ? styles.tab_active : ""} ${d.kind === "guard" ? styles.tab_guard : styles.tab_elicitation}`}
            onClick={() => setActiveDecision(d.id)}
          >
            {d.kind === "guard" ? (
              <svg className={styles.tab_icon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                <line x1="12" y1="9" x2="12" y2="13" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
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
          </button>
        ))}
      </div>
    </div>
  );
}
