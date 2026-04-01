import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useReportStore } from "../../store";
import type { DailyMetrics } from "../../types";
import styles from "./ReportView.module.css";

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

export function AISummaryCard({
  date,
  summary,
  metrics,
}: {
  date: string;
  summary: string | null;
  metrics: DailyMetrics;
}) {
  const { t } = useTranslation();
  const { generatingSummary, generateSummary } = useReportStore();
  const triggeredRef = useRef<string | null>(null);

  useEffect(() => {
    if (!summary && !generatingSummary && triggeredRef.current !== date) {
      triggeredRef.current = date;
      generateSummary(date);
    }
  }, [date, summary, generatingSummary, generateSummary]);

  // Split out the first paragraph as the hero line
  const { heroLine, bodyMarkdown } = useMemo(() => {
    if (!summary) return { heroLine: null, bodyMarkdown: null };
    const trimmed = summary.trim();
    // Skip leading heading (e.g. "# Daily Summary\n") if present
    let text = trimmed;
    if (/^#{1,3}\s/.test(text)) {
      const nlIdx = text.indexOf("\n");
      if (nlIdx !== -1) text = text.slice(nlIdx + 1).trimStart();
    }
    // First paragraph = everything up to the first blank line or next heading
    const splitMatch = text.match(/^(.*?)(?:\n\n|\n(?=#{1,3}\s))/s);
    if (splitMatch) {
      return {
        heroLine: splitMatch[1].trim(),
        bodyMarkdown: text.slice(splitMatch[0].length).trim(),
      };
    }
    return { heroLine: text, bodyMarkdown: null };
  }, [summary]);

  const totalTokens = metrics.totalInputTokens + metrics.totalOutputTokens;

  return (
    <div className={styles.section}>
      <h3 className={styles.section_title}>{t("report.ai_summary")}</h3>
      {summary ? (
        <div className={styles.summary_card}>
          {/* Hero area */}
          <div className={styles.summary_hero}>
            <div className={styles.summary_hero_text}>
              {heroLine && <ReactMarkdown remarkPlugins={[remarkGfm]}>{heroLine}</ReactMarkdown>}
            </div>
            <div className={styles.summary_hero_stats}>
              <div className={styles.hero_stat}>
                <span className={styles.hero_stat_value}>{metrics.totalSessions}</span>
                <span className={styles.hero_stat_label}>{t("report.sessions")}</span>
              </div>
              <div className={styles.hero_stat_divider} />
              <div className={styles.hero_stat}>
                <span className={styles.hero_stat_value}>{formatTokens(totalTokens)}</span>
                <span className={styles.hero_stat_label}>Tokens</span>
              </div>
              <div className={styles.hero_stat_divider} />
              <div className={styles.hero_stat}>
                <span className={styles.hero_stat_value}>{metrics.projects.length}</span>
                <span className={styles.hero_stat_label}>{t("report.projects")}</span>
              </div>
            </div>
          </div>
          {/* Body */}
          {bodyMarkdown && (
            <div className={styles.summary_content}>
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{bodyMarkdown}</ReactMarkdown>
            </div>
          )}
        </div>
      ) : (
        <div className={styles.summary_empty}>
          <p>{t("report.generating")}</p>
        </div>
      )}
    </div>
  );
}
