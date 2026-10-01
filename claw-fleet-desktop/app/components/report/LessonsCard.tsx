import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { safeMarkdownComponents, safeRemarkPlugins, safeRehypePlugins } from "../../markdown/safeLinks";
import { normalizeSvgBlankLines, markdownUrlTransform } from "../../markdown/plugins";
import { useReportStore } from "../../store";
import type { Lesson } from "../../types";
import styles from "./ReportView.module.css";
import { SkeletonList, Spinner } from "../loading";
import { usePending } from "../../hooks/usePending";

export function LessonsCard({
  date,
  lessons,
}: {
  date: string;
  lessons: Lesson[] | null;
}) {
  const { t } = useTranslation();
  const {
    generatingLessons,
    generateLessons,
    lessonsFailedDate,
    appendLessonToClaudeMd,
    managedLessons,
    managedLessonsLoaded,
    loadManagedLessons,
  } = useReportStore();
  // Same once-per-date trigger as the AI summary: a failed run must surface,
  // not leave the list on its loading placeholder.
  const failed = lessons === null && !generatingLessons && lessonsFailedDate === date;
  const triggeredRef = useRef<string | null>(null);

  useEffect(() => {
    if (lessons === null && !generatingLessons && triggeredRef.current !== date) {
      triggeredRef.current = date;
      generateLessons(date);
    }
  }, [date, lessons, generatingLessons, generateLessons]);

  // Real "already added" state: a lesson is added if the managed store holds a
  // block with the same session + content. Survives refresh, unlike local state.
  useEffect(() => {
    loadManagedLessons();
  }, [loadManagedLessons]);

  const isAdded = (lesson: Lesson) =>
    managedLessons.some(
      (m) => m.sessionId === lesson.sessionId && m.content === lesson.content,
    );

  return (
    <div className={styles.section}>
      <h3 className={styles.section_title}>{t("report.lessons")}</h3>
      {failed ? (
        <div className={styles.lessons_empty}>
          <p>{t("report.lessons_failed", "经验教训生成失败")}</p>
          <button className={styles.empty_retry_btn} onClick={() => generateLessons(date)}>
            {t("account.retry")}
          </button>
        </div>
      ) : lessons === null ? (
        <SkeletonList rows={3} rowHeight={64} />
      ) : lessons.length === 0 ? (
        <div className={styles.lessons_empty}>
          <p>{t("report.no_lessons_found")}</p>
        </div>
      ) : (
        <div className={styles.lessons_list}>
          {lessons.map((lesson, idx) => {
            const added = isAdded(lesson);
            return (
            <div key={idx} className={styles.lesson_card}>
              <div className={styles.lesson_content}>
                <div className={styles.lesson_text}><ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={safeMarkdownComponents}>{normalizeSvgBlankLines(lesson.content)}</ReactMarkdown></div>
                <div className={styles.lesson_reason}><ReactMarkdown urlTransform={markdownUrlTransform} remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={safeMarkdownComponents}>{normalizeSvgBlankLines(lesson.reason)}</ReactMarkdown></div>
                <div className={styles.lesson_meta}>
                  {lesson.workspaceName} · {lesson.sessionId}
                </div>
              </div>
              <LessonAddButton
                added={added}
                // Until the managed list answers, "added" is unknown: hold the
                // button in a pending state instead of offering a duplicate add.
                checking={!managedLessonsLoaded}
                onAdd={() => appendLessonToClaudeMd(lesson)}
              />
            </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function LessonAddButton({
  added,
  checking,
  onAdd,
}: {
  added: boolean;
  checking: boolean;
  onAdd: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const [pending, add] = usePending(onAdd);
  const busy = pending || checking;
  return (
    <button
      className={styles.lesson_add_btn}
      onClick={() => void add()}
      disabled={added || busy}
      aria-busy={busy || undefined}
    >
      {busy && !added && <Spinner size={12} />}
      {added ? t("report.lesson_added") : t("report.add_to_claude_md")}
    </button>
  );
}
