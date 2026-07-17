import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { safeMarkdownComponents, safeRemarkPlugins, safeRehypePlugins } from "../../markdown/safeLinks";
import { normalizeSvgBlankLines } from "../../markdown/plugins";
import { useReportStore } from "../../store";
import type { Lesson } from "../../types";
import styles from "./ReportView.module.css";

export function LessonsCard({
  date,
  lessons,
}: {
  date: string;
  lessons: Lesson[] | null;
}) {
  const { t } = useTranslation();
  const { generatingLessons, generateLessons, appendLessonToClaudeMd, managedLessons, loadManagedLessons } = useReportStore();
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

  const handleAdd = async (lesson: Lesson) => {
    await appendLessonToClaudeMd(lesson);
  };

  return (
    <div className={styles.section}>
      <h3 className={styles.section_title}>{t("report.lessons")}</h3>
      {lessons === null ? (
        <div className={styles.lessons_empty}>
          <p>{t("report.generating")}</p>
        </div>
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
                <div className={styles.lesson_text}><ReactMarkdown remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={safeMarkdownComponents}>{normalizeSvgBlankLines(lesson.content)}</ReactMarkdown></div>
                <div className={styles.lesson_reason}><ReactMarkdown remarkPlugins={safeRemarkPlugins} rehypePlugins={safeRehypePlugins} components={safeMarkdownComponents}>{normalizeSvgBlankLines(lesson.reason)}</ReactMarkdown></div>
                <div className={styles.lesson_meta}>
                  {lesson.workspaceName} · {lesson.sessionId}
                </div>
              </div>
              <button
                className={styles.lesson_add_btn}
                onClick={() => handleAdd(lesson)}
                disabled={added}
              >
                {added ? t("report.lesson_added") : t("report.add_to_claude_md")}
              </button>
            </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
