import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { CheckCircle2, ChevronDown, ChevronRight, TriangleAlert, XCircle } from "lucide-react";
import ReactMarkdown from "react-markdown";
import { safeMarkdownComponents, safeRemarkPlugins, safeRehypePlugins } from "../../markdown/safeLinks";
import { normalizeSvgBlankLines, markdownUrlTransform } from "../../markdown/plugins";
import { useReportStore } from "../../store";
import type { TaskReview } from "../../types";
import styles from "./ReportView.module.css";

/**
 * The day's per-task retrospectives — the visible end of the v3 decision card's
 * terminal state. Each row is one task that reached a terminal state on this
 * date (a handoff chain counts as one task), collapsed to its title and verdict;
 * expanding shows the review prose and the lessons drawn from it.
 *
 * Why this panel exists at all: the reviews were already being written to
 * `task_reviews` and folded into the day's lessons, but the review *prose* — and
 * in particular the disagreement between what the agent claimed and what the
 * user decided — had no surface. That disagreement is the single most
 * informative row here, so it gets its own badge rather than being left for the
 * reader to spot by comparing two fields.
 */
export function TaskReviewsCard({ date }: { date: string }) {
  const { t } = useTranslation();
  const { taskReviews, taskReviewsDate, loadTaskReviews } = useReportStore();
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  useEffect(() => {
    loadTaskReviews(date);
  }, [date, loadTaskReviews]);

  // Collapse everything when the date changes: a row left open would otherwise
  // reopen against a different task that happens to share the index.
  useEffect(() => {
    setExpanded(new Set());
  }, [date]);

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  // `taskReviewsDate !== date` means the fetch for this date is still in flight;
  // an empty array under a matching date is a real "nothing finished today".
  const loaded = taskReviewsDate === date;

  return (
    <div className={styles.section}>
      <h3 className={styles.section_title}>{t("report.task_reviews")}</h3>
      {!loaded ? (
        <div className={styles.lessons_empty}>
          <p>{t("report.loading")}</p>
        </div>
      ) : taskReviews.length === 0 ? (
        <div className={styles.lessons_empty}>
          <p>{t("report.no_task_reviews")}</p>
        </div>
      ) : (
        <div className={styles.lessons_list}>
          {taskReviews.map((r) => (
            <TaskReviewRow
              key={r.rootSessionId}
              review={r}
              open={expanded.has(r.rootSessionId)}
              onToggle={() => toggle(r.rootSessionId)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function TaskReviewRow({
  review,
  open,
  onToggle,
}: {
  review: TaskReview;
  open: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  const completed = review.outcome === "completed";
  // The agent's own verdict disagreed with the user's press. Both directions are
  // worth flagging, but they mean opposite things, so they get separate copy.
  const overclaimed = !completed && review.agentClaimedComplete;
  const underclaimed = completed && !review.agentClaimedComplete;
  const hops = review.sessionIds.length;

  return (
    <div className={styles.tr_row} data-outcome={review.outcome}>
      <button className={styles.tr_head} onClick={onToggle} aria-expanded={open}>
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        {completed ? (
          <CheckCircle2 size={14} className={styles.tr_icon_done} />
        ) : (
          <XCircle size={14} className={styles.tr_icon_given_up} />
        )}
        <span className={styles.tr_title}>
          {review.title || t("report.task_review_untitled")}
        </span>
        {overclaimed && (
          <span className={styles.tr_mismatch} title={t("report.task_review_overclaimed_hint")}>
            <TriangleAlert size={11} />
            {t("report.task_review_overclaimed")}
          </span>
        )}
        {underclaimed && (
          <span className={styles.tr_underclaim} title={t("report.task_review_underclaimed_hint")}>
            {t("report.task_review_underclaimed")}
          </span>
        )}
        <span className={styles.tr_meta}>
          {review.workspaceName}
          {/* Hop count only when the task actually handed off — "· 1 段" on every
              ordinary task would be noise on every row. */}
          {hops > 1 ? ` · ${t("report.task_review_hops", { count: hops })}` : ""}
        </span>
      </button>
      {open && (
        <div className={styles.tr_body}>
          <div className={styles.tr_summary}>
            <ReactMarkdown
              urlTransform={markdownUrlTransform}
              remarkPlugins={safeRemarkPlugins}
              rehypePlugins={safeRehypePlugins}
              components={safeMarkdownComponents}
            >
              {normalizeSvgBlankLines(review.summary)}
            </ReactMarkdown>
          </div>
          {review.lessons.length > 0 && (
            <ul className={styles.tr_lessons}>
              {review.lessons.map((l, i) => (
                <li key={i}>
                  <span className={styles.tr_lesson_content}>{l.content}</span>
                  <span className={styles.tr_lesson_reason}>{l.reason}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
