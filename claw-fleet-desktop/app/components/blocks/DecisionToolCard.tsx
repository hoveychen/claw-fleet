import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import { safeMarkdownComponents, safeRemarkPlugins, safeRehypePlugins } from "../../markdown/safeLinks";
import {
  normalizeAnswer,
  parseAnswersFromResultText,
  summarizeQuestion,
  type DecisionAnswer,
} from "../../decisionText";
import {
  asAskUserQuestionResult,
  asErrorMessage,
  type AskQuestion,
} from "../../toolResults";
import type {
  DecisionHistoryRecord,
  FleetAskFormField,
  FleetAskImage,
  ToolResultBlock,
  ToolUseBlock as ToolUseBlockType,
} from "../../types";
import { decisionAssetUrl } from "../../decisionAssets";
import { AutoHeightFrame } from "../AutoHeightFrame";
import { AttachmentRow } from "./AttachmentRow";
import styles from "./DecisionToolCard.module.css";

// Inline markdown: unwrap the <p> so option labels sit inside a <span>.
const inlineMarkdown: Components = {
  ...safeMarkdownComponents,
  p: ({ children }) => <>{children}</>,
};

/** A question as it appears in the `tool_use` input — the richest source, and
 *  the only one carrying `preview` / `html` / `images` / `formFields`. */
interface InputQuestion extends AskQuestion {
  html?: string;
  images?: FleetAskImage[];
  formFields?: FleetAskFormField[];
}

function readInputQuestions(input: Record<string, unknown>): InputQuestion[] {
  if (!Array.isArray(input.questions)) return [];
  const out: InputQuestion[] = [];
  for (const raw of input.questions) {
    if (typeof raw !== "object" || raw === null) continue;
    const q = raw as Record<string, unknown>;
    if (typeof q.question !== "string") continue;
    out.push({
      question: q.question,
      header: typeof q.header === "string" ? q.header : undefined,
      multiSelect: q.multiSelect === true,
      options: Array.isArray(q.options)
        ? q.options.flatMap((o) => {
            if (typeof o !== "object" || o === null) return [];
            const opt = o as Record<string, unknown>;
            if (typeof opt.label !== "string") return [];
            return [{
              label: opt.label,
              description: typeof opt.description === "string" ? opt.description : undefined,
              preview: typeof opt.preview === "string" ? opt.preview : undefined,
            }];
          })
        : undefined,
      html: typeof q.html === "string" ? q.html : undefined,
      images: Array.isArray(q.images) ? (q.images as FleetAskImage[]) : undefined,
      formFields: Array.isArray(q.formFields) ? (q.formFields as FleetAskFormField[]) : undefined,
    });
  }
  return out;
}

/**
 * Recover the user's answers, degrading through three sources.
 *
 * 1. `toolUseResult` — structured, and present on every non-errored call.
 * 2. The matching `list_session_decisions` record — the only source that also
 *    yields an **asset id**, without which a `fleet__ask` card's images cannot
 *    be re-served. Matched on the first question's text.
 * 3. The stringified `tool_result` content — brittle, for transcripts written
 *    before `toolUseResult` existed or by a non-Claude agent source.
 */
function resolveAnswers(
  questions: InputQuestion[],
  meta: unknown,
  record: DecisionHistoryRecord | null,
  result: ToolResultBlock | undefined,
): Record<string, string> {
  const fromMeta = asAskUserQuestionResult(meta);
  if (fromMeta && Object.keys(fromMeta.answers).length > 0) return fromMeta.answers;

  if (record && (record.kind === "fleet-ask" || record.kind === "elicitation")) {
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(record.answers)) {
      out[k] = typeof v === "string" ? v : v.label;
    }
    if (Object.keys(out).length > 0) return out;
  }

  if (result && typeof result.content === "string" && !result.is_error) {
    return parseAnswersFromResultText(result.content, questions);
  }
  return {};
}

/**
 * Whether this decision tool call has a shape the bespoke card can render.
 *
 * Callers must check before choosing this card: a malformed `questions` input
 * (a rejected call, a future schema) has to fall back to the generic tool block
 * rather than render nothing at all.
 */
export function hasDecisionQuestions(input: Record<string, unknown>): boolean {
  return readInputQuestions(input).length > 0;
}

/** Find the decision record this tool call produced, so images can be re-served. */
function matchRecord(
  questions: InputQuestion[],
  records: DecisionHistoryRecord[],
): DecisionHistoryRecord | null {
  const first = questions[0]?.question;
  if (!first) return null;
  return (
    records.find(
      (r) =>
        (r.kind === "elicitation" || r.kind === "fleet-ask") &&
        r.questions[0]?.question === first,
    ) ?? null
  );
}

interface Props {
  block: ToolUseBlockType;
  result?: ToolResultBlock;
  /** `toolUseResult` for this call, when the transcript carried one. */
  meta?: unknown;
  /** Decision records for the session; supplies asset ids for image cards. */
  records?: DecisionHistoryRecord[];
  isPartial?: boolean;
}

/**
 * A decision card (`AskUserQuestion` / `fleet__ask`) rendered inline in the
 * conversation.
 *
 * The generic tool card cannot show these: `formatInput` finds none of its
 * known keys on a `{questions: […]}` input and falls back to dumping
 * `JSON.stringify(input)` into the one-line header. Since AskUserQuestion is
 * among the most-used tools in a Fleet session, that blob is the single worst
 * thing in the transcript view.
 *
 * Collapsed by default with a human-readable header — a long session is mostly
 * tool calls, and an always-open card would dominate it.
 */
export function DecisionToolCard({ block, result, meta, records, isPartial }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const questions = useMemo(() => readInputQuestions(block.input), [block.input]);
  const record = useMemo(() => matchRecord(questions, records ?? []), [questions, records]);
  const answers = useMemo(
    () => resolveAnswers(questions, meta, record, result),
    [questions, meta, record, result],
  );

  const errorMessage = result?.is_error ? asErrorMessage(meta) ?? null : null;

  // Header gist: the first question's summary, plus what was chosen for it.
  const first = questions[0];
  const firstAnswer: DecisionAnswer | null = first
    ? normalizeAnswer(answers[first.question], (first.options ?? []).map((o) => o.label))
    : null;

  // Both AskUserQuestion and the fleet__ask MCP variant are "the agent asked a
  // question" — label them identically rather than leaking the raw tool name.
  const kindLabel = t("detail.decision_ask");

  // A card with no parseable questions is not worth a bespoke renderer.
  if (questions.length === 0) return null;

  return (
    <div className={`${styles.root} ${errorMessage ? styles.root_error : ""}`}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        <span className={styles.kind}>{kindLabel}</span>
        <span className={styles.summary}>{summarizeQuestion(first.question)}</span>
        {!open && firstAnswer && (
          <span className={styles.answer_chip} title={firstAnswer.label}>
            {/* Free text can be a paragraph; the chip states only that it was
                answered and defers the content to the expanded body. */}
            {firstAnswer.other ? t("detail.decision_answered_other") : firstAnswer.label}
          </span>
        )}
        {!open && !firstAnswer && !isPartial && !errorMessage && (
          <span className={styles.pending_chip}>{t("detail.decision_unanswered")}</span>
        )}
        {isPartial && !result && <span className={styles.spinner}>⟳</span>}
        {errorMessage && <span className={styles.error_badge}>error</span>}
        {questions.length > 1 && (
          <span className={styles.count}>{questions.length}</span>
        )}
      </button>

      {open && (
        <div className={styles.body}>
          {errorMessage && <pre className={styles.error_text}>{errorMessage}</pre>}

          {questions.map((q, qi) => {
            const labels = (q.options ?? []).map((o) => o.label);
            const answer = normalizeAnswer(answers[q.question], labels);
            const selected = new Set(
              answer && !answer.other
                ? answer.label.split(",").map((s) => s.trim()).filter(Boolean)
                : [],
            );

            return (
              <div key={qi} className={styles.question_block}>
                <div className={styles.question_text}>
                  <ReactMarkdown
                    remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins}
                    components={safeMarkdownComponents}
                  >
                    {q.question}
                  </ReactMarkdown>
                </div>

                {/* Image-bearing fleet__ask cards persist their assets, so the
                    exact preview can be re-served through fleet-decision://.
                    Without a matched record there is no asset id, so fall back
                    to a marker rather than replaying agent-authored HTML. */}
                {q.images && q.images.length > 0 && record ? (
                  <AutoHeightFrame
                    title={`decision-${record.id}-${qi}`}
                    className={styles.preview_frame}
                    src={decisionAssetUrl(record.id, `q${qi}`)}
                    minHeight={160}
                  />
                ) : (
                  (q.html || (q.images && q.images.length > 0)) && (
                    <div className={styles.html_marker}>
                      {t("decision_history.fleet_ask_html_marker", "[HTML preview was shown]")}
                    </div>
                  )
                )}

                {(q.options ?? []).map((opt, oi) => {
                  const isSelected = selected.has(opt.label);
                  return (
                    <div
                      key={oi}
                      className={`${styles.option} ${isSelected ? styles.option_selected : ""}`}
                    >
                      <span className={styles.option_label}>
                        <span className={styles.marker}>{isSelected ? "✓" : "○"}</span>
                        <ReactMarkdown remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={inlineMarkdown}>
                          {opt.label}
                        </ReactMarkdown>
                      </span>
                      {opt.description && (
                        <span className={styles.option_desc}>
                          <ReactMarkdown remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={inlineMarkdown}>
                            {opt.description}
                          </ReactMarkdown>
                        </span>
                      )}
                      {isSelected && opt.preview && (
                        <pre className={styles.option_preview}>{opt.preview}</pre>
                      )}
                    </div>
                  );
                })}

                {answer?.other && answer.label && (
                  <div className={`${styles.option} ${styles.option_selected}`}>
                    <span className={styles.option_label}>
                      <span className={styles.marker}>✓</span>
                      {t("decision_history.other_label", "Other")}
                    </span>
                    <span className={styles.option_desc}>{answer.label}</span>
                  </div>
                )}

                {(q.formFields ?? []).map((f, fi) => {
                  const v = answers[f.name];
                  if (!v) return null;
                  return (
                    <div key={`f-${fi}`} className={`${styles.option} ${styles.option_selected}`}>
                      <span className={styles.option_label}>
                        <span className={styles.marker}>▸</span>
                        {f.label || f.name}
                      </span>
                      <span className={styles.option_desc}>{v}</span>
                    </div>
                  );
                })}

                {answer && <AttachmentRow paths={answer.attachments} />}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
