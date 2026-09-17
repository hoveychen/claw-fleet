import { type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { markdownUrlTransform } from "../../markdown/plugins";
import type { DecisionHistoryRecord, RawMessage, ToolResultBlock } from "../../types";
import type { PathLinkContext } from "../../markdown/pathLinks";
import {
  safeMarkdownComponents,
  safeRemarkPlugins,
  safeRehypePlugins,
} from "../../markdown/safeLinks";
import { summarizeWorkRun, workRunFinished, workRunTitle } from "../workRuns";
import { formatMsgTime } from "../../messageRows";
import { ContentBlocks } from "./ContentBlocks";
import { RailDone } from "./Rail";
import { useBandOpen } from "./useBandOpen";
import styles from "./WorkRunBlock.module.css";

interface Props {
  /** The folded assistant records, in transcript order (≥ 2). */
  msgs: RawMessage[];
  resultMap: Map<string, ToolResultBlock>;
  metaMap: Map<string, unknown>;
  decisionRecords: DecisionHistoryRecord[];
  searchTerms?: string[] | null;
  paths?: PathLinkContext;
  /** True when this run is the transcript's trailing unit. The tail is where
   *  the reader is looking, so it starts open — folded, a growing tail shows
   *  only a rising step count and a newer timestamp with nothing to read. It
   *  *opens* the band and never closes it — see `useBandOpen` for why following
   *  it both ways made a live band flap. */
  defaultOpen: boolean;
  /** True while the session is in a working status. Drives the headline shimmer
   *  only; it must not gate `defaultOpen`, because the status drops out of the
   *  working set mid-run (a tool outliving the backend's 60s freshness window)
   *  and a band born in that gap would mount folded. */
  live?: boolean;
  /** True while the active search hit lives inside this run. Opens the band on
   *  the hit; stepping off leaves it open (same latch as `defaultOpen`). */
  forceOpen?: boolean;
}

/** Compact token count for the band tail: 843 → "843", 12 340 → "12.3k". */
function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
}

/** The band title is a thinking-derived headline that often carries markdown
 *  emphasis (`**Planning store test additions**`). Render it inline so the
 *  markers become real bold/italic/code instead of literal asterisks — `p`
 *  unwraps to a fragment so the single-sentence headline stays inline inside
 *  the header's nowrap/ellipsis span rather than becoming a block paragraph. */
export const titleMarkdownComponents = {
  ...safeMarkdownComponents,
  p: ({ children }: { children?: ReactNode }) => <>{children}</>,
};

/**
 * One collapsed band for a run of tool-call / thinking records between two
 * pieces of prose. The header states only derivable facts: a rule-mapped
 * category (`runCategory`), the step count, per-tool call counts, and the
 * summed output tokens. Expanding renders the member records' cards exactly
 * as they would have rendered unfolded.
 */
export function WorkRunBlock({
  msgs,
  resultMap,
  metaMap,
  decisionRecords,
  searchTerms,
  paths,
  defaultOpen,
  live,
  forceOpen,
}: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useBandOpen(defaultOpen, !!forceOpen);

  const summary = summarizeWorkRun(msgs);
  // A thinking-derived headline (the model's own summary sentence) beats the
  // rule-mapped category; runs with no thinking keep the category label.
  const title = workRunTitle(msgs);
  const finished = workRunFinished(msgs, !!live, (id) => resultMap.has(id));
  // In progress = the run can still grow, i.e. the same fact the Done check
  // reads, inverted. It used to additionally require the last record to be an
  // unterminated partial (`stop_reason === null`), which flapped the shimmer
  // off between records and for the entire time a tool was running — the same
  // stop_reason misreading that put a premature Done on the rail.
  const streaming = !finished && !!live && defaultOpen;
  const shimmer = streaming ? ` ${styles.shimmer}` : "";

  // The band collapses several records into one row, so show when the run
  // ended — the last member's timestamp — the way an unfolded row would.
  const lastTime = (() => {
    for (let i = msgs.length - 1; i >= 0; i--) {
      const ts = msgs[i]?.timestamp;
      if (ts) {
        const parsed = formatMsgTime(ts);
        if (parsed) return parsed;
      }
    }
    return null;
  })();

  return (
    <div className={styles.root}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        {title ? (
          <span className={`${styles.title}${shimmer}`}>
            <ReactMarkdown
              urlTransform={markdownUrlTransform}
              remarkPlugins={safeRemarkPlugins}
              rehypePlugins={safeRehypePlugins}
              components={titleMarkdownComponents}
            >
              {title}
            </ReactMarkdown>
          </span>
        ) : (
          <span className={`${styles.category}${shimmer}`}>{t(`detail.work_cat_${summary.category}`)}</span>
        )}
        {/* Step count, tokens and end time trail the headline as one dot-joined
            meta cluster — the same idiom the unfolded tool rows use. Anchoring
            them to the far right instead left them stranded across a wide
            reading column, reading as a second unrelated column. */}
        <span className={styles.meta}>
          {/* Just the step count — the per-tool breakdown reads better as the
              rail itself, one click away. */}
          <span>{t("detail.work_steps", { count: summary.steps })}</span>
          {summary.outputTokens > 0 && <span>· ↓{fmtTokens(summary.outputTokens)}</span>}
          {lastTime && (
            <span className={styles.time} title={lastTime.full}>
              · {lastTime.short}
            </span>
          )}
        </span>
      </button>
      {open && (
        <div className={styles.body}>
          {msgs.map((msg, i) => {
            const content = msg.message?.content;
            if (!Array.isArray(content)) return null;
            return (
              <ContentBlocks
                key={msg.uuid ?? i}
                content={content}
                resultMap={resultMap}
                metaMap={metaMap}
                decisionRecords={decisionRecords}
                isPartial={msg.message?.stop_reason === null && i === msgs.length - 1}
                searchTerms={searchTerms}
                paths={paths}
                rail
              />
            );
          })}
          {/* A finished run closes with the Done check; a run that can still
              grow (live tail, partial record, tool call with no result yet)
              keeps the rail open-ended — see `workRunFinished`. */}
          {finished && <RailDone />}
        </div>
      )}
    </div>
  );
}
