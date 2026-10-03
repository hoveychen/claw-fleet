import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { safeMarkdownComponents, safeRemarkPlugins, safeRehypePlugins } from "../../markdown/safeLinks";
import { normalizeSvgBlankLines, markdownUrlTransform } from "../../markdown/plugins";
import { useReportStore, useUIStore } from "../../store";
import type { DriftCheck, DriftVerdict, Lesson, LessonViolation } from "../../types";
import styles from "./ReportView.module.css";
import { SkeletonList, Spinner } from "../loading";
import { usePending } from "../../hooks/usePending";

/**
 * The day's "needs your judgment" items, and nothing else: relay chains an
 * outsider read flagged as drifting, lessons that recurred across sessions,
 * and adopted lessons that were broken again. Generation belongs to the
 * scheduler alone — this card only reads, so opening the page never kicks off
 * an LLM call against a half-finished day.
 */
export function AttentionCard({ date, showTitle = true }: { date: string; showTitle?: boolean }) {
  const { t } = useTranslation();
  const attention = useReportStore((s) => s.attention);
  const attentionDate = useReportStore((s) => s.attentionDate);
  const loadAttention = useReportStore((s) => s.loadAttention);

  useEffect(() => {
    void loadAttention(date);
  }, [date, loadAttention]);

  const loaded = attentionDate === date;
  const shown = loaded && attention?.date === date ? attention : null;
  const drift = shown?.drift ?? [];
  const lessons = shown?.lessons ?? [];
  const violations = shown?.violations ?? [];
  const empty = drift.length + lessons.length + violations.length === 0;

  return (
    <div className={styles.section}>
      {showTitle && <h3 className={styles.section_title}>{t("report.attention_title")}</h3>}
      {!loaded ? (
        <SkeletonList rows={2} rowHeight={64} />
      ) : empty ? (
        <div className={styles.lessons_empty}>
          <p>{t("report.attention_empty")}</p>
        </div>
      ) : (
        <div className={styles.lessons_list}>
          {drift.length > 0 && (
            <AttentionGroup label={t("report.attention_drift")}>
              {drift.map((d) => (
                <DriftRow key={d.chainId} check={d} />
              ))}
            </AttentionGroup>
          )}
          {lessons.length > 0 && <RecurringLessons lessons={lessons} />}
          {violations.length > 0 && (
            <AttentionGroup label={t("report.attention_violations")}>
              {violations.map((v) => (
                <ViolationRow key={v.lessonId} violation={v} />
              ))}
            </AttentionGroup>
          )}
        </div>
      )}
    </div>
  );
}

function Md({ text }: { text: string }) {
  return (
    <ReactMarkdown
      urlTransform={markdownUrlTransform}
      remarkPlugins={safeRemarkPlugins}
      rehypePlugins={safeRehypePlugins}
      components={safeMarkdownComponents}
    >
      {normalizeSvgBlankLines(text)}
    </ReactMarkdown>
  );
}

function AttentionGroup({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className={styles.att_group}>
      <div className={styles.att_group_label}>{label}</div>
      {children}
    </div>
  );
}

function OpenSessionButton({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  if (!sessionId) return null;
  return (
    <button
      className={styles.lesson_add_btn}
      onClick={() => useUIStore.getState().requestOpenTask(sessionId)}
    >
      {t("report.attention_open_session")}
    </button>
  );
}

const VERDICT_KEY: Record<DriftVerdict, string> = {
  on_track: "report.drift_on_track",
  polishing: "report.drift_polishing",
  goal_shifted: "report.drift_goal_shifted",
  unclear: "report.drift_unclear",
};

function DriftRow({ check }: { check: DriftCheck }) {
  const { t } = useTranslation();
  return (
    <div className={styles.lesson_card}>
      <div className={styles.lesson_content}>
        <div className={styles.att_head}>
          <span className={styles.att_verdict} data-verdict={check.verdict}>
            {t(VERDICT_KEY[check.verdict])}
          </span>
          <span className={styles.att_workspace}>{check.workspaceName}</span>
          <span className={styles.lesson_meta}>
            {t("report.drift_sessions", { count: check.sessionCount })}
          </span>
        </div>
        <div className={styles.att_goal} title={check.goal}>
          {t("report.drift_goal")}: {check.goal}
        </div>
        {/* The question is what the user decides on; the evidence backs it. */}
        {check.question && <div className={styles.att_question}>{check.question}</div>}
        {check.evidence && <div className={styles.lesson_reason}>{check.evidence}</div>}
      </div>
      <OpenSessionButton sessionId={check.latestSessionId} />
    </div>
  );
}

function RecurringLessons({ lessons }: { lessons: Lesson[] }) {
  const { t } = useTranslation();
  const { appendLessonToClaudeMd, managedLessons, managedLessonsLoaded, loadManagedLessons } =
    useReportStore();

  // Real "already added" state, read from ~/.claude/fleet-lessons.md so it
  // survives a refresh.
  useEffect(() => {
    void loadManagedLessons();
  }, [loadManagedLessons]);

  const isAdded = (lesson: Lesson) =>
    managedLessons.some((m) => m.sessionId === lesson.sessionId && m.content === lesson.content);

  return (
    <AttentionGroup label={t("report.attention_lessons")}>
      {lessons.map((lesson, idx) => (
        <div key={idx} className={styles.lesson_card}>
          <div className={styles.lesson_content}>
            <div className={styles.lesson_text}><Md text={lesson.content} /></div>
            <div className={styles.lesson_reason}><Md text={lesson.reason} /></div>
            <div className={styles.lesson_meta}>
              {lesson.workspaceName} ·{" "}
              {t("report.lesson_evidence", { count: lesson.evidenceSessionIds.length })}
            </div>
          </div>
          <LessonAddButton
            added={isAdded(lesson)}
            // Until the managed list answers, "added" is unknown: hold the
            // button in a pending state instead of offering a duplicate add.
            checking={!managedLessonsLoaded}
            onAdd={() => appendLessonToClaudeMd(lesson)}
          />
        </div>
      ))}
    </AttentionGroup>
  );
}

function ViolationRow({ violation }: { violation: LessonViolation }) {
  const { t } = useTranslation();
  return (
    <div className={styles.lesson_card}>
      <div className={styles.lesson_content}>
        <div className={styles.lesson_text}><Md text={violation.lessonContent} /></div>
        {violation.note && <div className={styles.lesson_reason}>{violation.note}</div>}
        <div className={styles.lesson_meta}>
          {t("report.lesson_evidence", { count: violation.sessionIds.length })}
        </div>
      </div>
      <OpenSessionButton sessionId={violation.sessionIds[0] ?? ""} />
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
