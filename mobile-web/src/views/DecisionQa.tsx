// The read-only question/answer body of a resolved decision card, shared by the
// two mobile surfaces that show one: the session's 决策 tab (`DecisionHistoryTab`,
// fed by decision-history records) and the 消息 tab's expanded tool step
// (`ToolDetailPanel`, fed by the transcript's `tool_use` input + result).
//
// Both used to render decision calls their own way — the 决策 tab with this
// layout, the 消息 tab not at all (its expanded body dumped the raw
// `{questions: […]}` JSON). One renderer keeps them from drifting; it is the
// mobile counterpart of the desktop's `DecisionToolCard` / `DecisionHistory`
// split over `decisionText.ts`.

import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { mermaidMarkdownComponents } from "../markdown/mermaidComponents";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import { splitAnswerAttachments } from "../userAttachments";
import { AttachmentThumbs } from "./AttachmentThumb";
import { stripTtsDivider } from "./decisionCall";
import styles from "./DecisionQa.module.css";

/** Links stay inert on a transcript surface — a tap must not navigate the
 *  webview away from the session. */
const mdLink: Components["a"] = ({ children }) => (
  <span className={styles.mdLink}>{children}</span>
);

export const MD_BLOCK: Components = { ...mermaidMarkdownComponents, a: mdLink };
export const MD_INLINE: Components = { ...MD_BLOCK, p: ({ children }) => <>{children}</> };

export function Md({ text, inline }: { text: string; inline?: boolean }) {
  return (
    <ReactMarkdown
      remarkPlugins={mdRemarkPlugins}
      rehypePlugins={mdRehypePlugins}
      components={inline ? MD_INLINE : MD_BLOCK}
    >
      {text}
    </ReactMarkdown>
  );
}

/** Option row with a ✓/○/▸ marker + markdown label/description, mirroring the
 *  desktop DecisionHistory option layout. */
export function OptionRow({
  label,
  description,
  selected,
  marker,
}: {
  label: string;
  description?: string | null;
  selected: boolean;
  marker?: string;
}) {
  return (
    <div className={styles.qaOption} data-picked={selected}>
      <span className={styles.qaOptionLabel}>
        <span className={styles.qaOptionMarker}>{marker ?? (selected ? "✓" : "○")}</span>
        <Md text={label} inline />
      </span>
      {description && (
        <span className={styles.qaOptionDesc}>
          <Md text={description} inline />
        </span>
      )}
    </div>
  );
}

/** The permissive question shape both feeds satisfy: a `FleetAskQuestion` off a
 *  decision-history record, and a `DecisionQuestion` parsed out of a transcript's
 *  `tool_use` input. */
export interface QaQuestion {
  question: string;
  options?: { label: string; description?: string | null }[];
  html?: string | null;
  images?: { name: string }[] | null;
  formFields?: { name: string; label: string }[];
}

/**
 * One resolved card's questions with what the user picked.
 *
 * `html` / `images` render as a marker rather than the real preview: those
 * assets are served out of the decision store against a *pending* request id, so
 * a resolved card has nothing left to fetch them with. (The desktop degrades the
 * same way.)
 */
export function DecisionQa({
  questions,
  answers,
  client,
}: {
  questions: QaQuestion[];
  answers: Record<string, string>;
  client: FleetTransport | null;
}) {
  return (
    <>
      {questions.map((q, i) => {
        // An answer can carry `@/path` mentions for the files the user attached.
        // They are part of the answer string, so they have to come off before
        // the label matching below — and they are what the thumbnails at the end
        // of the block render.
        const { core: raw, attachments } = splitAnswerAttachments(answers[q.question] ?? "");
        const opts = q.options ?? [];
        const fields = q.formFields ?? [];
        const picked = raw
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean);
        const matched = opts.some((o) => picked.includes(o.label));
        const isOther = !matched && raw.length > 0 && opts.length > 0;
        return (
          <div key={i} className={styles.qaBlock}>
            <div className={styles.qaQuestion}>
              <Md text={stripTtsDivider(q.question)} />
            </div>
            {q.images && q.images.length > 0 ? (
              <div className={styles.dimNote}>{t("[当时展示过图片预览]")}</div>
            ) : (
              q.html && <div className={styles.dimNote}>{t("[当时展示过 HTML 预览]")}</div>
            )}
            {opts.map((o, j) => (
              <OptionRow
                key={j}
                label={o.label}
                description={o.description}
                selected={picked.includes(o.label)}
              />
            ))}
            {isOther && <OptionRow label={t("其他")} description={raw} selected marker="✓" />}
            {fields.map((f, fi) => {
              const v = answers[f.name];
              if (v === undefined || v === "") return null;
              return (
                <OptionRow
                  key={`f-${fi}`}
                  label={f.label || f.name}
                  description={v}
                  selected
                  marker="▸"
                />
              );
            })}
            {opts.length === 0 && fields.length === 0 && raw && (
              <div className={styles.preWrap}>{raw}</div>
            )}
            <AttachmentThumbs paths={attachments} client={client} />
          </div>
        );
      })}
    </>
  );
}
