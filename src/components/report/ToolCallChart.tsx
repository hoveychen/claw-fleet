import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import styles from "./ReportView.module.css";

export function ToolCallChart({ breakdown }: { breakdown: Record<string, number> }) {
  const { t } = useTranslation();
  const sorted = useMemo(() => {
    return Object.entries(breakdown).sort((a, b) => b[1] - a[1]);
  }, [breakdown]);

  if (sorted.length === 0) return null;
  const max = sorted[0][1];

  return (
    <div className={styles.chart_card}>
      <h3 className={styles.chart_title}>{t("report.tool_breakdown")}</h3>
      <div className={styles.bar_chart}>
        {sorted.map(([tool, count]) => (
          <div key={tool} className={styles.bar_row}>
            <span className={styles.bar_label}>{tool}</span>
            <div className={styles.bar_track}>
              <div
                className={styles.bar_fill}
                style={{ width: `${(count / max) * 100}%` }}
              />
            </div>
            <span className={styles.bar_value}>{count}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
