import { useCallback, useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useReportStore } from "../../store";
import type { DailyReport } from "../../types";
import { ContributionsHeatmap } from "./ContributionsHeatmap";
import { HourlyActivityChart } from "./HourlyActivityChart";
import { MetricsCards } from "./MetricsCards";
import { AISummaryCard } from "./AISummaryCard";
import { LessonsCard } from "./LessonsCard";
import { ToolCallChart } from "./ToolCallChart";
import { ReportShareMenu } from "./ReportShareMenu";
import styles from "./ReportView.module.css";

// ── Helpers ─────────────────────────────────────────────────────────────────

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function formatDateLong(dateStr: string, locale?: string): string {
  const d = new Date(dateStr + "T00:00:00");
  return d.toLocaleDateString(locale, {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
}

// Strip the leading title/hero from aiSummary for a one-line preview.
function summaryPreview(aiSummary: string | null): string | null {
  if (!aiSummary) return null;
  let text = aiSummary.trim();
  if (/^#{1,3}\s/.test(text)) {
    const nlIdx = text.indexOf("\n");
    if (nlIdx !== -1) text = text.slice(nlIdx + 1).trimStart();
  }
  const firstChunk = text.split(/\n\n|\n(?=#{1,3}\s)/)[0] ?? "";
  return firstChunk.replace(/[#*`>_]/g, "").trim() || null;
}

// ── Component ───────────────────────────────────────────────────────────────

export function ReportView() {
  const { t } = useTranslation();
  const { loadHeatmap, loadReport, selectedDate } = useReportStore();

  useEffect(() => {
    const to = new Date().toISOString().slice(0, 10);
    const from = new Date(Date.now() - 365 * 86400000).toISOString().slice(0, 10);
    loadHeatmap(from, to);
  }, [loadHeatmap]);

  useEffect(() => {
    loadReport(selectedDate);
  }, [selectedDate, loadReport]);

  return (
    <div className={styles.root}>
      <header className={styles.header} data-tauri-drag-region>
        <h1 className={styles.title}>{t("report.panel_title")}</h1>
        <div className={styles.header_actions}>
          <ReportShareMenu />
        </div>
      </header>

      <div className={styles.heatmap_banner}>
        <ContributionsHeatmap />
      </div>

      <div className={styles.body}>
        <aside className={styles.list_pane}>
          <DateList />
        </aside>
        <main className={styles.detail_pane}>
          <ReportDetail />
        </main>
      </div>
    </div>
  );
}

// ── Date list (left pane) ───────────────────────────────────────────────────

function DateList() {
  const { t, i18n } = useTranslation();
  const {
    currentReport,
    selectedDate,
    timelineReports,
    timelineLoading,
    timelineHasMore,
    heatmapData,
    loadReport,
    loadTimelinePage,
    resetTimeline,
  } = useReportStore();
  const sentinelRef = useRef<HTMLDivElement>(null);

  // Reset & reload the timeline whenever the anchor date changes.
  useEffect(() => {
    if (heatmapData.length === 0) return;
    resetTimeline();
    loadTimelinePage();
  }, [selectedDate, heatmapData.length, resetTimeline, loadTimelinePage]);

  // Infinite scroll
  const handleIntersect = useCallback(
    (entries: IntersectionObserverEntry[]) => {
      if (entries[0]?.isIntersecting && timelineHasMore && !timelineLoading) {
        loadTimelinePage();
      }
    },
    [timelineHasMore, timelineLoading, loadTimelinePage],
  );

  useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const observer = new IntersectionObserver(handleIntersect, { rootMargin: "200px" });
    observer.observe(el);
    return () => observer.disconnect();
  }, [handleIntersect]);

  // Build the displayed list: selected day pinned at top + paginated timeline below.
  const displayed = useMemo<DailyReport[]>(() => {
    const out: DailyReport[] = [];
    if (currentReport) out.push(currentReport);
    for (const r of timelineReports) {
      if (currentReport && r.date === currentReport.date) continue;
      out.push(r);
    }
    return out;
  }, [currentReport, timelineReports]);

  if (displayed.length === 0 && timelineLoading) {
    return <p className={styles.empty}>{t("report.loading")}</p>;
  }

  return (
    <div className={styles.date_list}>
      {displayed.map((report) => (
        <DateCard
          key={report.date}
          report={report}
          active={report.date === selectedDate}
          locale={i18n.language}
          onClick={() => loadReport(report.date)}
        />
      ))}
      <div ref={sentinelRef} className={styles.list_sentinel}>
        {timelineLoading && <span className={styles.empty_inline}>{t("report.loading")}</span>}
        {!timelineHasMore && displayed.length > 1 && (
          <span className={styles.empty_inline}>·</span>
        )}
      </div>
    </div>
  );
}

function DateCard({
  report,
  active,
  locale,
  onClick,
}: {
  report: DailyReport;
  active: boolean;
  locale: string;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const totalTokens =
    report.metrics.totalInputTokens + report.metrics.totalOutputTokens;
  const lessonCount = report.lessons?.length ?? 0;
  const preview = summaryPreview(report.aiSummary);

  return (
    <button
      className={`${styles.date_card} ${active ? styles.date_card_active : ""}`}
      onClick={onClick}
    >
      <div className={styles.date_card_top}>
        <span className={styles.date_card_label}>
          {formatDateLong(report.date, locale)}
        </span>
        <span className={styles.date_card_iso}>{report.date}</span>
      </div>
      <div className={styles.date_card_chips}>
        <span className={styles.meta_chip}>
          {report.metrics.totalSessions} {t("report.sessions").toLowerCase()}
        </span>
        {totalTokens > 0 && (
          <span className={styles.meta_chip}>{formatTokens(totalTokens)} tok</span>
        )}
        {(report.metrics.totalCostUsd ?? 0) >= 0.005 && (
          <span className={styles.meta_chip}>
            ${(report.metrics.totalCostUsd ?? 0).toFixed(2)}
          </span>
        )}
        {lessonCount > 0 && (
          <span className={`${styles.meta_chip} ${styles.meta_chip_accent}`}>
            {lessonCount} {t("report.lessons").toLowerCase()}
          </span>
        )}
      </div>
      {preview && <div className={styles.date_card_preview}>{preview}</div>}
    </button>
  );
}

// ── Right-pane detail ───────────────────────────────────────────────────────

function ReportDetail() {
  const { t, i18n } = useTranslation();
  const { currentReport, selectedDate, loading, generateReport } = useReportStore();

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          <span className={styles.detail_workspace}>{t("report.panel_title")}</span>
          <span className={styles.detail_sep}>/</span>
          <span className={styles.detail_name}>
            {formatDateLong(selectedDate, i18n.language)}
          </span>
          <span className={styles.detail_iso}>{selectedDate}</span>
        </div>
      </div>

      <div className={styles.detail_body}>
        {loading && <div className={styles.empty}>{t("report.loading")}</div>}
        {!loading && !currentReport && (
          <div className={styles.empty_state}>
            <p>{t("report.no_report")}</p>
            <button
              className={styles.primary_btn}
              onClick={() => generateReport(selectedDate)}
            >
              {t("report.generate")}
            </button>
          </div>
        )}
        {!loading && currentReport && (
          <div className={styles.detail_content}>
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
            <LessonsCard date={currentReport.date} lessons={currentReport.lessons} />
          </div>
        )}
      </div>
    </>
  );
}

