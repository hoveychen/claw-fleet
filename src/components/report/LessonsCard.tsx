import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
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
  const { generatingLessons, generateLessons, appendLessonToClaudeMd } = useReportStore();
  const [added, setAdded] = useState<Set<number>>(new Set());
  const triggeredRef = useRef<string | null>(null);

  useEffect(() => {
    if (lessons === null && !generatingLessons && triggeredRef.current !== date) {
      triggeredRef.current = date;
      generateLessons(date);
    }
  }, [date, lessons, generatingLessons, generateLessons]);

  const handleAdd = async (lesson: Lesson, idx: number) => {
    await appendLessonToClaudeMd(lesson);
    setAdded((prev) => new Set(prev).add(idx));
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
          {lessons.map((lesson, idx) => (
            <div key={idx} className={styles.lesson_card}>
              <div className={styles.lesson_content}>
                <div className={styles.lesson_text}><ReactMarkdown remarkPlugins={[remarkGfm]}>{lesson.content}</ReactMarkdown></div>
                <div className={styles.lesson_reason}><ReactMarkdown remarkPlugins={[remarkGfm]}>{lesson.reason}</ReactMarkdown></div>
                <div className={styles.lesson_meta}>
                  {lesson.workspaceName} · {lesson.sessionId}
                </div>
              </div>
              <button
                className={styles.lesson_add_btn}
                onClick={() => handleAdd(lesson, idx)}
                disabled={added.has(idx)}
              >
                {added.has(idx) ? t("report.lesson_added") : t("report.add_to_claude_md")}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
