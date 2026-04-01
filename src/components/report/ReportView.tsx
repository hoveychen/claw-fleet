import { useCallback, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useReportStore } from "../../store";
import { ContributionsHeatmap } from "./ContributionsHeatmap";
import { HourlyActivityChart } from "./HourlyActivityChart";
import { InsightsTimeline } from "./InsightsTimeline";
import { MetricsCards } from "./MetricsCards";
import { AISummaryCard } from "./AISummaryCard";
import { LessonsCard } from "./LessonsCard";
import { ToolCallChart } from "./ToolCallChart";
import { ReportShareMenu } from "./ReportShareMenu";
import styles from "./ReportView.module.css";

export function ReportView() {
  const { t } = useTranslation();
  const {
    currentReport, selectedDate, loading, reportTab,
    loadReport, loadHeatmap, generateReport, setReportTab, resetTimeline,
  } = useReportStore();

  useEffect(() => {
    const to = new Date().toISOString().slice(0, 10);
    const from = new Date(Date.now() - 365 * 86400000).toISOString().slice(0, 10);
    loadHeatmap(from, to);
  }, [loadHeatmap]);

  // Load report when switching to daily tab or changing date
  useEffect(() => {
    if (reportTab === "daily") {
      loadReport(selectedDate);
    }
  }, [selectedDate, reportTab, loadReport]);

  const goToDate = useCallback((offset: number) => {
    const d = new Date(selectedDate);
    d.setDate(d.getDate() + offset);
    const s = useReportStore.getState();
    s.loadReport(d.toISOString().slice(0, 10));
  }, [selectedDate]);

  const switchToInsights = useCallback(() => {
    resetTimeline();
    setReportTab("insights");
  }, [resetTimeline, setReportTab]);

  return (
    <div className={styles.container}>
      {/* Tab bar */}
      <div className={styles.tab_bar} data-tauri-drag-region>
        <div className={styles.tab_group}>
          <button
            className={`${styles.tab} ${reportTab === "insights" ? styles.tab_active : ""}`}
            onClick={switchToInsights}
          >
            {t("report.tab_insights")}
          </button>
          <button
            className={`${styles.tab} ${reportTab === "daily" ? styles.tab_active : ""}`}
            onClick={() => setReportTab("daily")}
          >
            {t("report.tab_daily")}
          </button>
        </div>
        {reportTab === "daily" && (
          <div className={styles.date_nav}>
            <button className={styles.date_btn} onClick={() => goToDate(-1)}>◀</button>
            <input
              type="date"
              className={styles.date_input}
              value={selectedDate}
              onChange={(e) => loadReport(e.target.value)}
            />
            <button className={styles.date_btn} onClick={() => goToDate(1)}>▶</button>
            <ReportShareMenu />
          </div>
        )}
      </div>

      {reportTab === "insights" ? (
        <>
          <ContributionsHeatmap />
          <InsightsTimeline />
        </>
      ) : (
        <>
          <ContributionsHeatmap />
          {loading ? (
            <div className={styles.loading}>{t("report.loading")}</div>
          ) : !currentReport ? (
            <div className={styles.empty_state}>
              <p>{t("report.no_report")}</p>
              <button className={styles.generate_btn} onClick={() => generateReport(selectedDate)}>
                {t("report.generate")}
              </button>
            </div>
          ) : (
            <div className={styles.content}>
              <MetricsCards metrics={currentReport.metrics} />
              <div className={styles.charts_row}>
                <ToolCallChart breakdown={currentReport.metrics.toolCallBreakdown} />
                <HourlyActivityChart hourly={currentReport.metrics.hourlyActivity} />
              </div>
              <AISummaryCard
                date={currentReport.date}
                summary={currentReport.aiSummary}
                metrics={currentReport.metrics}
              />
              <LessonsCard
                date={currentReport.date}
                lessons={currentReport.lessons}
              />
            </div>
          )}
        </>
      )}
    </div>
  );
}
