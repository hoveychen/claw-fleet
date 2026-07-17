import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DecisionHistoryRecord, RawMessage, ToolResultBlock } from "../../types";
import type { PathLinkContext } from "../../markdown/pathLinks";
import { summarizeWorkRun, workRunTitle } from "../workRuns";
import { formatMsgTime } from "../../messageRows";
import { ContentBlocks } from "./ContentBlocks";
import { RailDone } from "./Rail";
import styles from "./WorkRunBlock.module.css";

interface Props {
  /** The folded assistant records, in transcript order (≥ 2). */
  msgs: RawMessage[];
  resultMap: Map<string, ToolResultBlock>;
  metaMap: Map<string, unknown>;
  decisionRecords: DecisionHistoryRecord[];
  searchTerms?: string[] | null;
  paths?: PathLinkContext;
  /** True while this run is the live tail of a working session. The band
   *  follows it both ways: open to show the tools streaming in, closed again
   *  once the agent moves on — a just-finished run tidies itself up. */
  defaultOpen: boolean;
  /** True while the active search hit lives inside this run. Like
   *  `defaultOpen`, the band follows the signal both ways — open on the hit,
   *  closed again once the reader steps off it. */
  forceOpen?: boolean;
}

/** Compact token count for the band tail: 843 → "843", 12 340 → "12.3k". */
function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
}

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
  forceOpen,
}: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(defaultOpen || !!forceOpen);
  useEffect(() => setOpen(defaultOpen || !!forceOpen), [defaultOpen, forceOpen]);

  const summary = summarizeWorkRun(msgs);
  // A thinking-derived headline (the model's own summary sentence) beats the
  // rule-mapped category; runs with no thinking keep the category label.
  const title = workRunTitle(msgs);
  const counts = summary.toolCounts
    .map(([name, c]) => `${name === "thinking" ? t("detail.work_thinking") : name}×${c}`)
    .join(" · ");

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
          <span className={styles.title}>{title}</span>
        ) : (
          <span className={styles.category}>{t(`detail.work_cat_${summary.category}`)}</span>
        )}
        <span className={styles.stats}>
          {t("detail.work_steps", { count: summary.steps })}
          {counts && ` · ${counts}`}
        </span>
        {summary.outputTokens > 0 && (
          <span className={styles.tokens}>↓{fmtTokens(summary.outputTokens)}</span>
        )}
        {lastTime && (
          <span className={styles.time} title={lastTime.full}>
            {lastTime.short}
          </span>
        )}
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
          {/* A finished run closes with the Done check; a still-streaming run
              (live tail, last record unterminated) keeps the rail open-ended. */}
          {msgs[msgs.length - 1]?.message?.stop_reason !== null && <RailDone />}
        </div>
      )}
    </div>
  );
}
