import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type {
  DecisionHistoryRecord,
  ElicitationHistoryRecord,
  PlanApprovalHistoryRecord,
  UserPromptHistoryRecord,
} from "../types";
import styles from "./DecisionHistory.module.css";

interface Props {
  sessionId: string;
  jsonlPath?: string;
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
            <div className={styles.question_text}>{q.question}</div>
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
                    {isSelected ? "✓" : "○"} {opt.label}
                  </span>
                  <span className={styles.option_desc}>{opt.description}</span>
                </div>
              );
            })}
            {selected?.other && (
              <div className={`${styles.option} ${styles.option_selected}`}>
                <span className={styles.option_label}>
                  ✓ {t("decision_history.other_label")}
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
      <pre className={styles.plan_content}>{rec.planContent}</pre>
      {rec.editedPlan && (
        <>
          <div className={styles.question_text}>
            {t("decision_history.edited_plan")}
          </div>
          <pre className={styles.plan_content}>{rec.editedPlan}</pre>
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

export function DecisionHistory({ sessionId, jsonlPath }: Props) {
  const { t } = useTranslation();
  const [records, setRecords] = useState<DecisionHistoryRecord[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [openId, setOpenId] = useState<string | null>(null);

  useEffect(() => {
    if (!sessionId) return;
    invoke<DecisionHistoryRecord[]>("list_session_decisions", {
      sessionId,
      jsonlPath: jsonlPath ?? null,
    })
      .then((r) => setRecords(r ?? []))
      .catch(() => setRecords([]));
  }, [sessionId, jsonlPath]);

  if (records.length === 0) return null;

  // Backend returns oldest-first; show newest-first in the panel.
  const ordered = [...records].sort((a, b) =>
    recordTimestamp(b).localeCompare(recordTimestamp(a))
  );

  return (
    <div className={styles.root}>
      <div className={styles.header} onClick={() => setExpanded((v) => !v)}>
        <span className={styles.title}>{t("decision_history.title")}</span>
        <span className={styles.count}>{records.length}</span>
        <span className={styles.chevron}>{expanded ? "▾" : "▸"}</span>
      </div>
      {expanded && (
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
