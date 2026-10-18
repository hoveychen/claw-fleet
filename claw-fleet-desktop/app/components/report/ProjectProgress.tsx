import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import type { ProjectMetrics } from "../../types";
import { useUIStore } from "../../store";
import { projectDays } from "./reviewDays";
import styles from "./ReportView.module.css";

/** Tasks shown per project before the rest fold into a count. */
const TASKS_PER_PROJECT = 6;

/**
 * 回顾's main section: what moved in each project that day — the tasks worked
 * there, each with the last thing its agent said, which is where a task's
 * progress reads. A row opens the task.
 */
export function ProjectProgress({ projects }: { projects: ProjectMetrics[] }) {
  const { t } = useTranslation();
  const days = useMemo(() => projectDays(projects), [projects]);
  if (days.length === 0) return null;

  return (
    <div className={styles.section}>
      <h3 className={styles.section_title}>{t("review.by_project", "各项目进展")}</h3>
      {days.map((day) => {
        const shown = day.tasks.slice(0, TASKS_PER_PROJECT);
        const rest = day.tasks.length - shown.length;
        return (
          <div key={day.path} className={styles.project_card} title={day.path}>
            <div className={styles.project_header}>
              <span className={styles.project_name}>{day.name}</span>
              <span className={styles.project_stats}>
                {t("review.task_count", "{{count}} 个任务", { count: day.tasks.length })}
                {day.subtaskCount > 0 && ` · ${t("review.subtask_count", "{{count}} 个子任务", { count: day.subtaskCount })}`}
                {day.costUsd >= 0.005 && ` · $${day.costUsd.toFixed(2)}`}
              </span>
            </div>
            <div className={styles.session_list}>
              {shown.map((s) => (
                <button
                  key={s.id}
                  type="button"
                  className={styles.progress_row}
                  onClick={() => useUIStore.getState().requestOpenTask(s.id)}
                >
                  <span className={styles.progress_title}>{s.title || t("review.untitled", "（无标题任务）")}</span>
                  {s.lastMessage && <span className={styles.progress_last}>{s.lastMessage}</span>}
                </button>
              ))}
              {rest > 0 && (
                <div className={styles.progress_more}>{t("review.more_tasks", "还有 {{count}} 个任务", { count: rest })}</div>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}
