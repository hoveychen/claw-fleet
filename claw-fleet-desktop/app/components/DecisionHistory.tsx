import { useState, useMemo } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import { safeRemarkPlugins, safeRehypePlugins } from "../markdown/safeLinks";
import { usePathMarkdown } from "../hooks/usePathLinks";
import { normalizeAnswer, summarizeQuestion } from "../decisionText";
import type {
  DecisionHistoryRecord,
  ElicitationHistoryRecord,
  FleetAskHistoryRecord,
  PlanApprovalHistoryRecord,
  UserPromptHistoryRecord,
} from "../types";
import { History } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { AttachmentRow } from "./blocks/AttachmentRow";
import { decisionAssetUrl } from "../decisionAssets";
import { AutoHeightFrame } from "./AutoHeightFrame";
import styles from "./DecisionHistory.module.css";

/**
 * Block + inline markdown variants for one record, with that workspace's paths
 * clickable. Records carry their own sessionId, so each body resolves its own
 * workspace rather than inheriting one from the list.
 *
 * The inline variant unwraps the surrounding <p> so rendered output can sit
 * inside the <span>s used for option labels/descriptions without producing
 * invalid HTML or unwanted block margins.
 */
function useRecordMarkdown(sessionId: string): { block: Components; inline: Components } {
  const block = usePathMarkdown(sessionId);
  return useMemo(
    () => ({ block, inline: { ...block, p: ({ children }) => <>{children}</> } }),
    [block],
  );
}

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
    case "cancelled":
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
  const md = useRecordMarkdown(rec.sessionId);
  return (
    <div className={styles.body}>
      {rec.questions.map((q, qi) => {
        const selected = rec.answers[q.question];
        return (
          <div key={qi} className={styles.question_block}>
            <div className={styles.question_text}>
              <ReactMarkdown
                remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
                components={md.block}
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
                      remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
                      components={md.inline}
                    >
                      {opt.label}
                    </ReactMarkdown>
                  </span>
                  {opt.description && (
                    <span className={styles.option_desc}>
                      <ReactMarkdown
                        remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
                        components={md.inline}
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
  const md = useRecordMarkdown(rec.sessionId);
  return (
    <div className={styles.body}>
      <div className={styles.plan_content}>
        <ReactMarkdown
          remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
          components={md.block}
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
              remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
              components={md.block}
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

/**
 * Render a v2 fleet__ask history record. Mirrors ElicitationBody's shape
 * (question + option highlight + "Other" treatment) and additionally
 * surfaces form-field answers and `@path` attachment mentions. The
 * original `html` field is shown as a plain "[HTML preview was shown]"
 * marker — we deliberately do NOT re-render it as a sandboxed iframe in
 * history view to avoid replaying arbitrary HTML the agent emitted.
 */
function FleetAskBody({ rec }: { rec: FleetAskHistoryRecord }) {
  const { t } = useTranslation();
  const md = useRecordMarkdown(rec.sessionId);

  return (
    <div className={styles.body}>
      {rec.questions.map((q, qi) => {
        // `normalizeAnswer` peels `@path` mention suffixes off the answer and
        // decides option-vs-"Other"; shared with the inline DecisionToolCard.
        const answer = normalizeAnswer(rec.answers[q.question], (q.options ?? []).map((o) => o.label));
        const answerCore = answer?.label ?? "";
        const paths = answer?.attachments ?? [];
        const isOther = answer?.other ?? false;
        const selectedLabels = answerCore
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s.length > 0);

        return (
          <div key={qi} className={styles.question_block}>
            <div className={styles.question_text}>
              <ReactMarkdown
                remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
                components={md.block}
              >
                {q.question}
              </ReactMarkdown>
            </div>
            {q.images && q.images.length > 0 ? (
              // Image-bearing cards persist their assets under
              // ~/.fleet/decision-assets/<id>/q<qi>/, so we CAN faithfully
              // re-render the exact preview through the fleet-decision://
              // protocol (still sandboxed, opaque origin). This is the whole
              // point of copying images into a durable store.
              <AutoHeightFrame
                title={`fleet-ask-history-${rec.id}-${qi}`}
                src={decisionAssetUrl(rec.id, `q${qi}`)}
                minHeight={160}
                style={{
                  width: "100%",
                  border: "1px solid var(--decision-card-border, #ccc)",
                  borderRadius: "0.4rem",
                  background: "#fff",
                }}
              />
            ) : (
              q.html && (
                <div className={styles.option_desc}>
                  {t("decision_history.fleet_ask_html_marker", "[HTML preview was shown]")}
                </div>
              )
            )}
            {(q.options ?? []).map((opt, oi) => {
              const isSelected = !isOther && selectedLabels.includes(opt.label);
              return (
                <div
                  key={oi}
                  className={`${styles.option} ${isSelected ? styles.option_selected : ""}`}
                >
                  <span className={styles.option_label}>
                    <span className={styles.option_marker}>{isSelected ? "✓" : "○"}</span>
                    <ReactMarkdown
                      remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
                      components={md.inline}
                    >
                      {opt.label}
                    </ReactMarkdown>
                  </span>
                  {opt.description && (
                    <span className={styles.option_desc}>
                      <ReactMarkdown
                        remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
                        components={md.inline}
                      >
                        {opt.description}
                      </ReactMarkdown>
                    </span>
                  )}
                </div>
              );
            })}
            {isOther && answerCore && (
              <div className={`${styles.option} ${styles.option_selected}`}>
                <span className={styles.option_label}>
                  <span className={styles.option_marker}>✓</span>
                  {t("decision_history.other_label")}
                </span>
                <span className={styles.option_desc}>{answerCore}</span>
              </div>
            )}
            {(q.formFields ?? []).map((f, fi) => {
              const v = rec.answers[f.name];
              if (v === undefined || v === "") return null;
              return (
                <div key={`f-${fi}`} className={`${styles.option} ${styles.option_selected}`}>
                  <span className={styles.option_label}>
                    <span className={styles.option_marker}>▸</span>
                    {f.label || f.name}
                  </span>
                  <span className={styles.option_desc}>{v}</span>
                </div>
              );
            })}
            <AttachmentRow paths={paths} />
          </div>
        );
      })}
    </div>
  );
}

function recordSummary(rec: DecisionHistoryRecord): string {
  if (rec.kind === "elicitation" || rec.kind === "fleet-ask") {
    const first = rec.questions[0];
    if (!first) return rec.kind === "fleet-ask" ? "fleet__ask" : "AskUserQuestion";
    return summarizeQuestion(first.question);
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
        <EmptyState
          icon={<History size={28} strokeWidth={1.5} />}
          title={t("empty_state.decision_title")}
          subtitle={t("empty_state.decision_subtitle")}
        />
      )}
      {showList && records.length > 0 && (
        <div className={styles.list}>
          {ordered.map((rec) => {
            const open = openId === rec.id;
            const isPlan = rec.kind === "plan-approval";
            const isUser = rec.kind === "user-prompt";
            const isFleetAsk = rec.kind === "fleet-ask";
            const kindKey = isUser
              ? "decision_history.kind_user"
              : isPlan
              ? "decision_history.kind_plan"
              : isFleetAsk
              ? "decision_history.kind_fleet_ask"
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
              >
                <div
                  className={styles.row_head}
                  onClick={() => setOpenId(open ? null : rec.id)}
                >
                  <span className={`${styles.kind_chip} ${kindClass}`}>
                    {t(kindKey, isFleetAsk ? { defaultValue: "fleet__ask" } : undefined)}
                  </span>
                  {!isUser && (
                    <span
                      className={`${styles.outcome_chip} ${outcomeClass(rec.outcome)}`}
                    >
                      {t(`decision_history.outcome.${rec.outcome}`, isFleetAsk && rec.outcome === "cancelled" ? { defaultValue: "Cancelled" } : undefined)}
                    </span>
                  )}
                  <span className={styles.summary}>{recordSummary(rec)}</span>
                  <span className={styles.time}>{fmtTime(recordTimestamp(rec))}</span>
                </div>
                {open && rec.kind === "elicitation" && <ElicitationBody rec={rec} />}
                {open && rec.kind === "plan-approval" && <PlanApprovalBody rec={rec} />}
                {open && rec.kind === "user-prompt" && <UserPromptBody rec={rec} />}
                {open && rec.kind === "fleet-ask" && <FleetAskBody rec={rec} />}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
