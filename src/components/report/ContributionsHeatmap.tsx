import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useReportStore } from "../../store";
import type { DailyReportStats } from "../../types";
import styles from "./ContributionsHeatmap.module.css";

// Color levels for the heatmap
function getLevel(value: number, max: number): number {
  if (value === 0) return 0;
  if (max === 0) return 0;
  const ratio = value / max;
  if (ratio < 0.25) return 1;
  if (ratio < 0.5) return 2;
  if (ratio < 0.75) return 3;
  return 4;
}

// Generate all dates in the last year as YYYY-MM-DD
function generateDateGrid(): string[][] {
  const weeks: string[][] = [];
  const today = new Date();
  // Start from the Sunday of 52 weeks ago
  const start = new Date(today);
  start.setDate(start.getDate() - start.getDay() - 52 * 7);

  const current = new Date(start);
  while (current <= today) {
    const week: string[] = [];
    for (let d = 0; d < 7; d++) {
      if (current <= today) {
        week.push(current.toISOString().slice(0, 10));
      }
      current.setDate(current.getDate() + 1);
    }
    weeks.push(week);
  }
  return weeks;
}

// Get month labels with their column positions
function getMonthLabels(weeks: string[][]): { label: string; col: number }[] {
  const labels: { label: string; col: number }[] = [];
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  let lastMonth = -1;
  for (let w = 0; w < weeks.length; w++) {
    const firstDate = weeks[w][0];
    if (!firstDate) continue;
    const month = new Date(firstDate).getMonth();
    if (month !== lastMonth) {
      labels.push({ label: months[month], col: w });
      lastMonth = month;
    }
  }
  return labels;
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return String(n);
}

export function ContributionsHeatmap() {
  const { t } = useTranslation();
  const { heatmapData, loadReport, selectedDate } = useReportStore();
  const [tooltip, setTooltip] = useState<{ date: string; stats: DailyReportStats | null; x: number; y: number } | null>(null);
  const [metric, setMetric] = useState<"tokens" | "sessions" | "toolCalls">("tokens");

  const dataMap = useMemo(() => {
    const map = new Map<string, DailyReportStats>();
    for (const s of heatmapData) map.set(s.date, s);
    return map;
  }, [heatmapData]);

  const weeks = useMemo(() => generateDateGrid(), []);
  const monthLabels = useMemo(() => getMonthLabels(weeks), [weeks]);

  const maxValue = useMemo(() => {
    let max = 0;
    for (const s of heatmapData) {
      const v = metric === "tokens" ? s.totalTokens : metric === "sessions" ? s.totalSessions : s.totalToolCalls;
      if (v > max) max = v;
    }
    return max;
  }, [heatmapData, metric]);

  const getValue = (stats: DailyReportStats | undefined): number => {
    if (!stats) return 0;
    if (metric === "tokens") return stats.totalTokens;
    if (metric === "sessions") return stats.totalSessions;
    return stats.totalToolCalls;
  };

  const dayLabels = ["", "Mon", "", "Wed", "", "Fri", ""];

  return (
    <div className={styles.container}>
      <div className={styles.header}>
        <h3 className={styles.title}>{t("report.contributions")}</h3>
        <div className={styles.metric_toggle}>
          <button className={metric === "tokens" ? styles.active : ""} onClick={() => setMetric("tokens")}>{t("report.tokens")}</button>
          <button className={metric === "sessions" ? styles.active : ""} onClick={() => setMetric("sessions")}>{t("report.sessions_label")}</button>
          <button className={metric === "toolCalls" ? styles.active : ""} onClick={() => setMetric("toolCalls")}>{t("report.tool_calls")}</button>
        </div>
      </div>

      {/* Month labels */}
      <div className={styles.month_labels}>
        <div className={styles.day_label_spacer} />
        <div className={styles.month_labels_track}>
          {monthLabels.map((m) => (
            <div key={`${m.label}-${m.col}`} className={styles.month_label} style={{ left: `${(m.col / weeks.length) * 100}%` }}>{m.label}</div>
          ))}
        </div>
      </div>

      {/* Grid */}
      <div className={styles.grid_wrapper}>
        <div className={styles.day_labels}>
          {dayLabels.map((l, i) => (
            <div key={i} className={styles.day_label}>{l}</div>
          ))}
        </div>
        <div className={styles.grid}>
          {weeks.map((week, wi) =>
            week.map((date, di) => {
              const stats = dataMap.get(date);
              const value = getValue(stats);
              const level = getLevel(value, maxValue);
              const isSelected = date === selectedDate;
              return (
                <div
                  key={date}
                  className={`${styles.cell} ${styles[`level_${level}`]} ${isSelected ? styles.selected : ""}`}
                  style={{ gridColumn: wi + 1, gridRow: di + 1 }}
                  onClick={() => loadReport(date)}
                  onMouseEnter={(e) => {
                    const rect = e.currentTarget.getBoundingClientRect();
                    setTooltip({ date, stats: stats || null, x: rect.left, y: rect.top - 40 });
                  }}
                  onMouseLeave={() => setTooltip(null)}
                />
              );
            })
          )}
        </div>
      </div>

      {/* Legend */}
      <div className={styles.legend}>
        <span className={styles.legend_label}>{t("report.less")}</span>
        {[0, 1, 2, 3, 4].map((l) => (
          <div key={l} className={`${styles.cell} ${styles[`level_${l}`]} ${styles.legend_cell}`} />
        ))}
        <span className={styles.legend_label}>{t("report.more")}</span>
      </div>

      {/* Tooltip */}
      {tooltip && (
        <div className={styles.tooltip} style={{ left: tooltip.x, top: tooltip.y }}>
          <strong>{tooltip.date}</strong>
          {tooltip.stats ? (
            <div>
              {formatTokens(tooltip.stats.totalTokens)} tokens, {tooltip.stats.totalSessions} sessions
            </div>
          ) : (
            <div>{t("report.no_data")}</div>
          )}
        </div>
      )}
    </div>
  );
}
