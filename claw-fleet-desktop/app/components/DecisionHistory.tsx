import { useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { safeMarkdownComponents } from "../markdown/safeLinks";
import type {
  DecisionHistoryRecord,
  ElicitationHistoryRecord,
  PlanApprovalHistoryRecord,
  UserPromptHistoryRecord,
} from "../types";
import styles from "./DecisionHistory.module.css";

// Inline-only markdown variant: unwraps the surrounding <p> so we can drop the
// rendered output inside <span>s (option label/desc) without producing invalid
// HTML or unwanted block margins.
const inlineMarkdownComponents: Components = {
  ...safeMarkdownComponents,
  p: ({ children }) => <>{children}</>,
};

interface Props {
  records: DecisionHistoryRecord[];
  /**
   * "inline" (default): collapsible header, fits between Skill history and
   * the message scroll. "tab": no header, list is always expanded — used
   * when the parent view renders this as a full panel inside a tab.
   */
  mode?: "inline" | "tab";
}

function recordTimestamp(rec: DecisionHistoryRecord): string {
  if (rec.kind === "user-prompt") return rec.sentAt;
  return rec.requestedAt;
}

function fmtTime(iso: string): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function outcomeClass(outcome: string): string {
  switch (outcome) {
    case "answered":
      return styles.outcome_answered;
    case "declined":
      return styles.outcome_declined;
    case "timeout":
      return styles.outcome_timeout;
    case "heartbeat-lost":
      return styles.outcome_heartbeat_lost;
    case "approved":
      return styles.outcome_approved;
    case "approved-with-edits":
      return styles.outcome_approved_with_edits;
    case "rejected":
      return styles.outcome_rejected;
    default:
      return "";
  }
}

function ElicitationBody({ rec }: { rec: ElicitationHistoryRecord }) {
  const { t } = useTranslation();
  return (
    <div className={styles.body}>
      {rec.questions.map((q, qi) => {
        const selected = rec.answers[q.question];
        return (
          <div key={qi} className={styles.question_block}>
            <div className={styles.question_text}>
              <ReactMarkdown
                remarkPlugins={[remarkGfm]}
                components={safeMarkdownComponents}
              >
                {q.question}
              </ReactMarkdown>
            </div>
            {q.options.map((opt, oi) => {
              const isSelected =
                selected != null &&
                !selected.other &&
                selected.label.split(",").map((s) => s.trim()).includes(opt.label);
              return (
                <div
                  key={oi}
                  className={`${styles.option} ${isSelected ? styles.option_selected : ""}`}
                >
                  <span className={styles.option_label}>
                    <span className={styles.option_marker}>{isSelected ? "✓" : "○"}</span>
                    <ReactMarkdown
                      remarkPlugins={[remarkGfm]}
                      components={inlineMarkdownComponents}
                    >
                      {opt.label}
                    </ReactMarkdown>
                  </span>
                  {opt.description && (
                    <span className={styles.option_desc}>
                      <ReactMarkdown
                        remarkPlugins={[remarkGfm]}
                        components={inlineMarkdownComponents}
                      >
                        {opt.description}
                      </ReactMarkdown>
                    </span>
                  )}
                </div>
              );
            })}
            {selected?.other && (
              <div className={`${styles.option} ${styles.option_selected}`}>
                <span className={styles.option_label}>
                  <span className={styles.option_marker}>✓</span>
                  {t("decision_history.other_label")}
                </span>
                <span className={styles.option_desc}>{selected.label}</span>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function UserPromptBody({ rec }: { rec: UserPromptHistoryRecord }) {
  const { t } = useTranslation();
  return (
    <div className={styles.body}>
      <pre className={styles.user_prompt_text}>{rec.text}</pre>
      {rec.hasImage && (
        <div className={styles.user_prompt_image_note}>
          {t("decision_history.has_image")}
        </div>
      )}
    </div>
  );
}

function PlanApprovalBody({ rec }: { rec: PlanApprovalHistoryRecord }) {
  const { t } = useTranslation();
  return (
    <div className={styles.body}>
      <div className={styles.plan_content}>
        <ReactMarkdown
          remarkPlugins={[remarkGfm]}
          components={safeMarkdownComponents}
        >
          {rec.planContent}
        </ReactMarkdown>
      </div>
      {rec.editedPlan && (
        <>
          <div className={styles.question_text}>
            {t("decision_history.edited_plan")}
          </div>
          <div className={styles.plan_content}>
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={safeMarkdownComponents}
            >
              {rec.editedPlan}
            </ReactMarkdown>
          </div>
        </>
      )}
      {rec.feedback && (
        <div className={styles.feedback}>
          {t("decision_history.feedback", { text: rec.feedback })}
        </div>
      )}
    </div>
  );
}

function recordSummary(rec: DecisionHistoryRecord): string {
  if (rec.kind === "elicitation") {
    const first = rec.questions[0];
    if (!first) return "AskUserQuestion";
    // Strip the "Speech Summary Divider" preamble if present.
    const body = first.question;
    const m = body.match(/^\s*---\s*$/m);
    return (m && m.index !== undefined ? body.slice(0, m.index) : body)
      .trim()
      .slice(0, 80);
  }
  if (rec.kind === "user-prompt") {
    return rec.text.replace(/\s+/g, " ").trim().slice(0, 80);
  }
  return rec.aiTitle ?? rec.workspaceName ?? "Plan approval";
}

export function DecisionHistory({ records, mode = "inline" }: Props) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [openId, setOpenId] = useState<string | null>(null);

  // Inline mode: hide the panel entirely when no records exist (preserves
  // the original chrome-light behavior). Tab mode: render an empty state
  // because the tab itself is always present.
  if (mode === "inline" && records.length === 0) return null;

  // Show oldest-first so the list reads chronologically as the session evolved.
  const ordered = [...records].sort((a, b) =>
    recordTimestamp(a).localeCompare(recordTimestamp(b))
  );

  const isTab = mode === "tab";
  const showList = isTab || expanded;

  return (
    <div className={`${styles.root} ${isTab ? styles.root_tab : ""}`}>
      {!isTab && (
        <div className={styles.header} onClick={() => setExpanded((v) => !v)}>
          <span className={styles.title}>{t("decision_history.title")}</span>
          <span className={styles.count}>{records.length}</span>
          <span className={styles.chevron}>{expanded ? "▾" : "▸"}</span>
        </div>
      )}
      {isTab && records.length === 0 && (
        <div className={styles.empty}>{t("decision_history.empty")}</div>
      )}
      {showList && records.length > 0 && (
        <div className={styles.list}>
          {ordered.map((rec) => {
            const open = openId === rec.id;
            const isPlan = rec.kind === "plan-approval";
            const isUser = rec.kind === "user-prompt";
            const kindKey = isUser
              ? "decision_history.kind_user"
              : isPlan
              ? "decision_history.kind_plan"
              : "decision_history.kind_ask";
            const kindClass = isUser
              ? styles.kind_chip_user
              : isPlan
              ? styles.kind_chip_plan
              : "";
            return (
              <div
                key={rec.id}
                className={`${styles.row} ${open ? styles.row_open : ""}`}
                onClick={() => setOpenId(open ? null : rec.id)}
              >
                <div className={styles.row_head}>
                  <span className={`${styles.kind_chip} ${kindClass}`}>
                    {t(kindKey)}
                  </span>
                  {!isUser && (
                    <span
                      className={`${styles.outcome_chip} ${outcomeClass(rec.outcome)}`}
                    >
                      {t(`decision_history.outcome.${rec.outcome}`)}
                    </span>
                  )}
                  <span className={styles.summary}>{recordSummary(rec)}</span>
                  <span className={styles.time}>{fmtTime(recordTimestamp(rec))}</span>
                </div>
                {open && rec.kind === "elicitation" && <ElicitationBody rec={rec} />}
                {open && rec.kind === "plan-approval" && <PlanApprovalBody rec={rec} />}
                {open && rec.kind === "user-prompt" && <UserPromptBody rec={rec} />}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
