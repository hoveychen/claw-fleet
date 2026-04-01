import { useCallback, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useReportStore } from "../../store";
import type { DailyReport, Lesson } from "../../types";
import styles from "./ReportView.module.css";

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function formatDate(dateStr: string): string {
  const d = new Date(dateStr + "T00:00:00");
  return d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

function TimelineEntry({ report, onOpenDaily }: { report: DailyReport; onOpenDaily: (date: string) => void }) {
  const { t } = useTranslation();
  const { appendLessonToClaudeMd } = useReportStore();
  const totalTokens = report.metrics.totalInputTokens + report.metrics.totalOutputTokens;

  const handleAddLesson = async (lesson: Lesson, btn: HTMLButtonElement) => {
    await appendLessonToClaudeMd(lesson);
    btn.textContent = t("report.lesson_added");
    btn.disabled = true;
  };

  // Split out hero line from summary (same logic as AISummaryCard)
  let heroLine: string | null = null;
  let bodyMarkdown: string | null = null;
  if (report.aiSummary) {
    let text = report.aiSummary.trim();
    if (/^#{1,3}\s/.test(text)) {
      const nlIdx = text.indexOf("\n");
      if (nlIdx !== -1) text = text.slice(nlIdx + 1).trimStart();
    }
    const splitMatch = text.match(/^(.*?)(?:\n\n|\n(?=#{1,3}\s))/s);
    if (splitMatch) {
      heroLine = splitMatch[1].trim();
      bodyMarkdown = text.slice(splitMatch[0].length).trim();
    } else {
      heroLine = text;
    }
  }

  return (
    <div className={styles.timeline_entry}>
      {/* Date header */}
      <button className={styles.timeline_date} onClick={() => onOpenDaily(report.date)}>
        <span className={styles.timeline_date_text}>{formatDate(report.date)}</span>
        <span className={styles.timeline_date_detail}>{report.date}</span>
        <span className={styles.timeline_date_arrow}>→</span>
      </button>

      {/* AI Summary card (reuses the redesigned summary_card styles) */}
      {report.aiSummary ? (
        <div className={styles.summary_card}>
          <div className={styles.summary_hero}>
            <div className={styles.summary_hero_text}>
              {heroLine && <ReactMarkdown remarkPlugins={[remarkGfm]}>{heroLine}</ReactMarkdown>}
            </div>
            <div className={styles.summary_hero_stats}>
              <div className={styles.hero_stat}>
                <span className={styles.hero_stat_value}>{report.metrics.totalSessions}</span>
                <span className={styles.hero_stat_label}>{t("report.sessions")}</span>
              </div>
              <div className={styles.hero_stat_divider} />
              <div className={styles.hero_stat}>
                <span className={styles.hero_stat_value}>{formatTokens(totalTokens)}</span>
                <span className={styles.hero_stat_label}>Tokens</span>
              </div>
              <div className={styles.hero_stat_divider} />
              <div className={styles.hero_stat}>
                <span className={styles.hero_stat_value}>{report.metrics.projects.length}</span>
                <span className={styles.hero_stat_label}>{t("report.projects")}</span>
              </div>
            </div>
          </div>
          {bodyMarkdown && (
            <div className={styles.summary_content}>
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{bodyMarkdown}</ReactMarkdown>
            </div>
          )}
        </div>
      ) : (
        <div className={styles.summary_empty}>{t("report.no_summary")}</div>
      )}

      {/* Lessons */}
      {report.lessons && report.lessons.length > 0 && (
        <div className={styles.timeline_lessons}>
          <h4 className={styles.timeline_lessons_title}>{t("report.lessons")}</h4>
          <div className={styles.lessons_list}>
            {report.lessons.map((lesson, idx) => (
              <div key={idx} className={styles.lesson_card}>
                <div className={styles.lesson_content}>
                  <div className={styles.lesson_text}><ReactMarkdown remarkPlugins={[remarkGfm]}>{lesson.content}</ReactMarkdown></div>
                  <div className={styles.lesson_reason}><ReactMarkdown remarkPlugins={[remarkGfm]}>{lesson.reason}</ReactMarkdown></div>
                  <div className={styles.lesson_meta}>{lesson.workspaceName}</div>
                </div>
                <button
                  className={styles.lesson_add_btn}
                  onClick={(e) => handleAddLesson(lesson, e.currentTarget)}
                >
                  {t("report.add_to_claude_md")}
                </button>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

export function InsightsTimeline() {
  const { t } = useTranslation();
  const { timelineReports, timelineLoading, timelineHasMore, heatmapData, loadTimelinePage, loadReport } = useReportStore();
  const sentinelRef = useRef<HTMLDivElement>(null);

  // Load first page when heatmap data is ready and timeline is empty
  useEffect(() => {
    if (heatmapData.length > 0 && timelineReports.length === 0 && !timelineLoading) {
      loadTimelinePage();
    }
  }, [heatmapData, timelineReports.length, timelineLoading, loadTimelinePage]);

  // Infinite scroll via IntersectionObserver
  const handleIntersect = useCallback(
    (entries: IntersectionObserverEntry[]) => {
      if (entries[0]?.isIntersecting && timelineHasMore && !timelineLoading) {
        loadTimelinePage();
      }
    },
    [timelineHasMore, timelineLoading, loadTimelinePage]
  );

  useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const observer = new IntersectionObserver(handleIntersect, { rootMargin: "200px" });
    observer.observe(el);
    return () => observer.disconnect();
  }, [handleIntersect]);

  const handleOpenDaily = useCallback((date: string) => {
    loadReport(date);
  }, [loadReport]);

  if (timelineReports.length === 0 && timelineLoading) {
    return <div className={styles.loading}>{t("report.loading")}</div>;
  }

  if (timelineReports.length === 0 && !timelineLoading) {
    return <div className={styles.empty_state}><p>{t("report.no_insights")}</p></div>;
  }

  return (
    <div className={styles.timeline}>
      {timelineReports.map((report) => (
        <TimelineEntry key={report.date} report={report} onOpenDaily={handleOpenDaily} />
      ))}
      <div ref={sentinelRef} className={styles.timeline_sentinel}>
        {timelineLoading && <span className={styles.loading}>{t("report.loading")}</span>}
      </div>
    </div>
  );
}
