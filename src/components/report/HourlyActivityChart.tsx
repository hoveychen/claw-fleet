import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import styles from "./ReportView.module.css";

export function HourlyActivityChart({ hourly }: { hourly: number[] }) {
  const { t } = useTranslation();
  const max = useMemo(() => Math.max(...hourly, 1), [hourly]);

  return (
    <div className={styles.chart_card}>
      <h3 className={styles.chart_title}>{t("report.hourly_activity")}</h3>
      <div className={styles.hourly_chart}>
        {hourly.map((count, hour) => (
          <div key={hour} className={styles.hourly_col}>
            <div className={styles.hourly_bar_wrapper}>
              <div
                className={styles.hourly_bar}
                style={{ height: `${(count / max) * 100}%` }}
                title={`${hour}:00 - ${count} sessions`}
              />
            </div>
            <span className={styles.hourly_label}>{hour % 6 === 0 ? `${hour}` : ""}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
