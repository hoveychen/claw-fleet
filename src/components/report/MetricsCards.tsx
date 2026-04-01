import { useTranslation } from "react-i18next";
import type { DailyMetrics } from "../../types";
import styles from "./ReportView.module.css";

function formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

export function MetricsCards({ metrics }: { metrics: DailyMetrics }) {
  const { t } = useTranslation();
  const totalTokens = metrics.totalInputTokens + metrics.totalOutputTokens;

  const cards = [
    { label: t("report.total_tokens"), value: formatNumber(totalTokens), sub: `${formatNumber(metrics.totalInputTokens)} in / ${formatNumber(metrics.totalOutputTokens)} out` },
    { label: t("report.sessions"), value: String(metrics.totalSessions), sub: `${metrics.totalSubagents} ${t("report.subagents")}` },
    { label: t("report.tool_calls"), value: String(metrics.totalToolCalls), sub: `${Object.keys(metrics.toolCallBreakdown).length} ${t("report.tool_types")}` },
    { label: t("report.projects"), value: String(metrics.projects.length), sub: `${Object.keys(metrics.sourceBreakdown).length} ${t("report.sources")}` },
  ];

  return (
    <div className={styles.metrics_row}>
      {cards.map((c) => (
        <div key={c.label} className={styles.metric_card}>
          <div className={styles.metric_value}>{c.value}</div>
          <div className={styles.metric_label}>{c.label}</div>
          <div className={styles.metric_sub}>{c.sub}</div>
        </div>
      ))}
    </div>
  );
}
