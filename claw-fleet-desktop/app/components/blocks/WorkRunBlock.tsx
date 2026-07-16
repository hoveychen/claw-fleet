import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DecisionHistoryRecord, RawMessage, ToolResultBlock } from "../../types";
import type { PathLinkContext } from "../../markdown/pathLinks";
import { summarizeWorkRun } from "../workRuns";
import { ContentBlocks } from "./ContentBlocks";
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
  const counts = summary.toolCounts
    .map(([name, c]) => `${name === "thinking" ? t("detail.work_thinking") : name}×${c}`)
    .join(" · ");

  return (
    <div className={styles.root}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        <span className={styles.category}>{t(`detail.work_cat_${summary.category}`)}</span>
        <span className={styles.stats}>
          {t("detail.work_steps", { count: summary.steps })}
          {counts && ` · ${counts}`}
        </span>
        {summary.outputTokens > 0 && (
          <span className={styles.tokens}>↓{fmtTokens(summary.outputTokens)}</span>
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
              />
            );
          })}
        </div>
      )}
    </div>
  );
}
