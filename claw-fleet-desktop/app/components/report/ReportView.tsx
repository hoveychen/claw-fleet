import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useReportStore } from "../../store";
import { localDateKey, localDateKeyDaysAgo } from "../../localDate";
import type { DailyReport } from "../../types";
import { ContributionsHeatmap } from "./ContributionsHeatmap";
import { HourlyActivityChart } from "./HourlyActivityChart";
import { MetricsCards } from "./MetricsCards";
import { DecisionCardsPanel } from "./DecisionCardsPanel";
import { TaskReviewsCard } from "./TaskReviewsCard";
import { AttentionCard } from "./AttentionCard";
import { ToolCallChart } from "./ToolCallChart";
import { ReportShareMenu } from "./ReportShareMenu";
import { BarChart3, RefreshCw, Copy } from "lucide-react";
import { EmptyState } from "../EmptyState";
import { PageShell } from "../PageShell";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "../ContextMenu";
import styles from "./ReportView.module.css";
import { Presence } from "../Presence";
import { SkeletonCard, SkeletonList, Spinner, TopProgress, loadingStyles } from "../loading";
import { useDelayedFlag } from "../../hooks/useDelayedFlag";

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

// ── Component ───────────────────────────────────────────────────────────────

export function ReportView() {
  const { t } = useTranslation();
  const { loadHeatmap, loadReport, selectedDate, markReportSeen } = useReportStore();

  useEffect(() => {
    // Entering the report view clears the "new report" red dot. Clear
    // immediately for snappy feedback, then reconcile against the freshly
    // loaded heatmap (which sets the true latest date).
    markReportSeen();
    const to = localDateKey();
    const from = localDateKeyDaysAgo(365);
    loadHeatmap(from, to).then(() => markReportSeen());
  }, [loadHeatmap, markReportSeen]);

  useEffect(() => {
    loadReport(selectedDate);
  }, [selectedDate, loadReport]);

  return (
    <PageShell
      view="report"
      detailKey={selectedDate}
      title={t("report.panel_title")}
      actions={<ReportShareMenu />}
      secondary={
        <div className={styles.list_pane}>
          <div className={styles.list_pane_heatmap}>
            <ContributionsHeatmap compact />
          </div>
          <DateList />
        </div>
      }
    >
      <ReportDetail />
    </PageShell>
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
    heatmapLoaded,
    loadReport,
    loadTimelinePage,
    resetTimeline,
    generateReport,
  } = useReportStore();
  const sentinelRef = useRef<HTMLDivElement>(null);
  // True from mount until the first timeline page has been asked for and
  // answered. `timelineLoading` alone stays false until the heatmap lands, which
  // left the list blank before it switched to its loading state.
  const [firstPagePending, setFirstPagePending] = useState(true);
  // Row whose "regenerate" is in flight, so the clicked row itself shows it.
  const [regenDate, setRegenDate] = useState<string | null>(null);
  const footerBusy = useDelayedFlag(timelineLoading);

  // Row context menu — anchor + subject held together, mirroring WikiView.
  const [ctxMenu, setCtxMenu] = useState<{ report: DailyReport; anchor: ContextMenuAnchor } | null>(
    null,
  );
  const menuItems = (report: DailyReport): ContextMenuItem[] => [
    {
      id: "regen",
      label:
        (report.metrics.totalSessions ?? 0) > 0
          ? t("report.regenerate", "重新生成报告")
          : t("report.generate", "生成报告"),
      icon: <RefreshCw size={13} strokeWidth={1.7} />,
      onSelect: () => {
        setRegenDate(report.date);
        void generateReport(report.date).finally(() =>
          setRegenDate((d) => (d === report.date ? null : d)),
        );
      },
    },
    {
      id: "copy-date",
      label: t("report.copy_date", "复制日期"),
      icon: <Copy size={13} strokeWidth={1.7} />,
      sub: report.date,
      onSelect: () => void writeText(report.date).catch(() => {}),
    },
  ];

  // Build the timeline once heatmap data is available. The timeline itself is
  // independent of which day is selected — selection only affects which card
  // gets the active highlight (and, in the edge case below, a pinned copy).
  useEffect(() => {
    if (heatmapData.length === 0) {
      // Nothing to page through once the heatmap has answered empty.
      if (heatmapLoaded) setFirstPagePending(false);
      return;
    }
    resetTimeline();
    void loadTimelinePage().finally(() => setFirstPagePending(false));
  }, [heatmapData.length, heatmapLoaded, resetTimeline, loadTimelinePage]);

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

  // The selected day is highlighted in place via the active prop. We only pin
  // a separate copy at the top when the selection isn't in the loaded timeline
  // yet (e.g. user clicked a heatmap cell that's past the current page).
  const displayed = useMemo<DailyReport[]>(() => {
    const inTimeline =
      currentReport != null &&
      timelineReports.some((r) => r.date === currentReport.date);
    const out: DailyReport[] = [];
    if (currentReport && !inTimeline) out.push(currentReport);
    for (const r of timelineReports) out.push(r);
    return out;
  }, [currentReport, timelineReports]);

  if (displayed.length === 0 && (!heatmapLoaded || firstPagePending || timelineLoading)) {
    return <SkeletonList rows={8} rowHeight={64} />;
  }

  return (
    <div className={styles.date_list}>
      {displayed.map((report) => (
        <DateCard
          key={report.date}
          report={report}
          active={report.date === selectedDate}
          busy={report.date === regenDate}
          locale={i18n.language}
          onClick={() => loadReport(report.date)}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
            setCtxMenu({ report, anchor: { x: e.clientX, y: e.clientY } });
          }}
        />
      ))}
      <Presence when={Boolean(ctxMenu)}>{ctxMenu && (
        <ContextMenu
          anchor={ctxMenu.anchor}
          items={menuItems(ctxMenu.report)}
          onClose={() => setCtxMenu(null)}
        />
      )}</Presence>
      <div ref={sentinelRef} className={styles.list_sentinel}>
        {footerBusy && displayed.length > 0 && <Spinner size={12} label={t("loading")} />}
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
  busy,
  locale,
  onClick,
  onContextMenu,
}: {
  report: DailyReport;
  active: boolean;
  /** A regenerate for this row is in flight. */
  busy: boolean;
  locale: string;
  onClick: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  const { t } = useTranslation();
  const totalTokens =
    report.metrics.totalInputTokens + report.metrics.totalOutputTokens;
  const lessonCount = report.lessons?.length ?? 0;

  return (
    <button
      className={`${styles.date_card} ${active ? styles.date_card_active : ""}`}
      onClick={onClick}
      onContextMenu={onContextMenu}
    >
      <div className={styles.date_card_top}>
        <span className={styles.date_card_label}>
          {formatDateLong(report.date, locale)}
        </span>
        <span className={styles.date_card_iso}>{report.date}</span>
        {busy && <Spinner size={12} className={styles.date_card_spinner} />}
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
    </button>
  );
}

// ── Right-pane detail ───────────────────────────────────────────────────────

/** The selected day's report body. Exported so the auto-popup overlay shows
 *  the exact same thing the page does instead of a second, drifting layout. */
export function ReportDetail() {
  const { t, i18n } = useTranslation();
  const { currentReport, selectedDate, loading, reportSettledDate, generateReport } =
    useReportStore();
  // Only a report for the selected day may stay on screen during a load: a
  // regenerate keeps it (dimmed, under a progress bar), but a date switch must
  // not show the previous day's numbers under the new day's header.
  const shown = currentReport && currentReport.date === selectedDate ? currentReport : null;
  const refreshing = useDelayedFlag(loading && shown !== null);
  // "No report" is only true once a load for this date has finished; before
  // that (including the first render, before `loading` flips on) it is a wait.
  const settled = !loading && reportSettledDate === selectedDate;

  return (
    <>
      <div className={styles.detail_header}>
        <TopProgress active={refreshing} />
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
        {!shown && !settled && (
          <div className={styles.detail_content}>
            <SkeletonCard height={72} />
            <div className={styles.charts_row}>
              <SkeletonCard height={180} />
              <SkeletonCard height={180} />
            </div>
            <SkeletonCard height={160} />
          </div>
        )}
        {!shown && settled && (
          <EmptyState
            icon={<BarChart3 size={28} strokeWidth={1.5} />}
            title={t("empty_state.report_title")}
            subtitle={t("empty_state.report_subtitle")}
            action={{
              label: t("report.generate"),
              onClick: () => generateReport(selectedDate),
            }}
          />
        )}
        {shown && (
          <div className={`${styles.detail_content} ${loading ? loadingStyles.stale : ""}`}>
            {/* First on purpose: the only part of the day that asks the
                user for a decision. Everything below is reference numbers. */}
            <AttentionCard date={shown.date} />
            <MetricsCards metrics={shown.metrics} />
            <div className={styles.charts_row}>
              <ToolCallChart breakdown={shown.metrics.toolCallBreakdown} />
              <HourlyActivityChart hourly={shown.metrics.hourlyActivity} />
            </div>
            <DecisionCardsPanel stats={shown.metrics.decisionCards} />
            {/* After the card stats: they say how many tasks ended, this
                says which ones and why. */}
            <TaskReviewsCard date={shown.date} />
          </div>
        )}
      </div>
    </>
  );
}

