// "Needs your judgment": the day's drifting relay chains, recurring lessons
// and adopted lessons broken again, read from the desktop over the relay
// (`daily_attention`). Today and yesterday are merged — drift checks land on
// the day they run, lessons on the day they were drawn from (yesterday) — so
// the page answers "what is waiting on me now" without a date picker.
//
// Fullscreen overlay from "More", same z-level as WikiView; the HistoryLayer
// wrapping it in App owns the back gesture.

import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, RefreshCw, WifiOff } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { DailyAttention, DriftCheck, DriftVerdict, Lesson, LessonViolation } from "../types";
import styles from "./AttentionView.module.css";
import { AppHeader } from "./AppHeader";
import { HeaderAction } from "./HeaderAction";
import { SkeletonList, Spinner } from "./loading";
import { useDelayedFlag } from "../useDelayedFlag";
import { usePending } from "../usePending";

interface Props {
  client: FleetTransport | null;
  onOpenSession: (sessionId: string) => void;
  onBack: () => void;
}

/** Local calendar date `daysAgo` days back, as `YYYY-MM-DD`. */
function localDate(daysAgo: number): string {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** Merge several days' items: newest drift check per chain wins; lessons and
 *  violations are de-duplicated by content / lesson id. */
export function mergeAttention(days: DailyAttention[]): {
  drift: DriftCheck[];
  lessons: Lesson[];
  violations: LessonViolation[];
} {
  const drift = new Map<string, DriftCheck>();
  const lessons = new Map<string, Lesson>();
  const violations = new Map<string, LessonViolation>();
  for (const day of days) {
    for (const d of day.drift ?? []) {
      const prev = drift.get(d.chainId);
      if (!prev || d.checkedAt > prev.checkedAt) drift.set(d.chainId, d);
    }
    for (const l of day.lessons ?? []) if (!lessons.has(l.content)) lessons.set(l.content, l);
    for (const v of day.violations ?? []) if (!violations.has(v.lessonId)) violations.set(v.lessonId, v);
  }
  return {
    drift: [...drift.values()].sort((a, b) => b.checkedAt - a.checkedAt),
    lessons: [...lessons.values()],
    violations: [...violations.values()],
  };
}

// Thunks so each label stays a literal t() call the i18n key test can see.
const VERDICT_LABEL: Record<DriftVerdict, () => string> = {
  on_track: () => t("方向正常"),
  polishing: () => t("原地打磨"),
  goal_shifted: () => t("目标偏移"),
  unclear: () => t("看不清"),
};

export function AttentionView({ client, onOpenSession, onBack }: Props) {
  const [items, setItems] = useState<ReturnType<typeof mergeAttention> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const refresh = useCallback(async () => {
    if (!client) return;
    setError(null);
    setRefreshing(true);
    try {
      const days = await Promise.all(
        [0, 1].map((n) =>
          client.request<DailyAttention>("daily_attention", { date: localDate(n) }),
        ),
      );
      setItems(mergeAttention(days.filter(Boolean)));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setRefreshing(false);
    }
  }, [client]);
  const showRefreshing = useDelayedFlag(refreshing);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const adopt = useCallback(
    async (lesson: Lesson) => {
      if (!client) return;
      await client.request("adopt_lesson", { lesson });
      // Adopted lessons drop out of the list on the desktop side.
      await refresh();
    },
    [client, refresh],
  );

  const total = items ? items.drift.length + items.lessons.length + items.violations.length : 0;

  return (
    <div className={styles.page}>
      <AppHeader
        onBack={onBack}
        title={t("需要你判断的事")}
        titleAfter={total > 0 && <span className={styles.count}>{total}</span>}
        actions={
          <HeaderAction
            icon={<RefreshCw size={17} />}
            label={t("刷新")}
            onClick={() => void refresh()}
            busy={showRefreshing}
            disabled={refreshing}
          />
        }
      />

      <div className={styles.view}>
        {error && <div className={styles.hint}>{t("加载失败：{0}", error)}</div>}
        {!error && items === null &&
          (client ? <SkeletonList rows={4} /> : <EmptyState icon={WifiOff} title={t("桌面端离线")} />)}
        {!error && items !== null && total === 0 && (
          <EmptyState
            icon={CheckCircle2}
            title={t("今天没有需要你判断的事。")}
            description={t("接力链跑偏、教训在多个会话里反复出现时，会出现在这里。")}
          />
        )}

        {items && items.drift.length > 0 && (
          <section className={styles.group}>
            <div className={styles.groupLabel}>{t("可能跑偏的接力链")}</div>
            {items.drift.map((d) => (
              <div key={d.chainId} className={styles.card}>
                <div className={styles.head}>
                  <span className={styles.verdict} data-verdict={d.verdict}>
                    {VERDICT_LABEL[d.verdict]()}
                  </span>
                  <span className={styles.workspace}>{d.workspaceName}</span>
                  <span className={styles.meta}>{t("{0} 棒", d.sessionCount)}</span>
                </div>
                <div className={styles.goal}>
                  {t("目标")}: {d.goal}
                </div>
                {d.question && <div className={styles.text}>{d.question}</div>}
                {d.evidence && <div className={styles.reason}>{d.evidence}</div>}
                {d.latestSessionId && (
                  <div className={styles.actions}>
                    <button className={styles.btn} onClick={() => onOpenSession(d.latestSessionId)}>
                      {t("打开会话")}
                    </button>
                  </div>
                )}
              </div>
            ))}
          </section>
        )}

        {items && items.lessons.length > 0 && (
          <section className={styles.group}>
            <div className={styles.groupLabel}>{t("反复出现的教训")}</div>
            {items.lessons.map((l) => (
              <div key={l.content} className={styles.card}>
                <div className={styles.text}>{l.content}</div>
                <div className={styles.reason}>{l.reason}</div>
                <div className={styles.meta}>
                  {l.workspaceName} · {t("{0} 个会话出现", l.evidenceSessionIds.length)}
                </div>
                <div className={styles.actions}>
                  <AdoptButton onAdopt={() => adopt(l)} />
                </div>
              </div>
            ))}
          </section>
        )}

        {items && items.violations.length > 0 && (
          <section className={styles.group}>
            <div className={styles.groupLabel}>{t("已采纳但又被违反的教训")}</div>
            {items.violations.map((v) => (
              <div key={v.lessonId} className={styles.card}>
                <div className={styles.text}>{v.lessonContent}</div>
                {v.note && <div className={styles.reason}>{v.note}</div>}
                <div className={styles.meta}>{t("{0} 个会话出现", v.sessionIds.length)}</div>
                {v.sessionIds[0] && (
                  <div className={styles.actions}>
                    <button className={styles.btn} onClick={() => onOpenSession(v.sessionIds[0])}>
                      {t("打开会话")}
                    </button>
                  </div>
                )}
              </div>
            ))}
          </section>
        )}
      </div>
    </div>
  );
}

function AdoptButton({ onAdopt }: { onAdopt: () => Promise<void> }) {
  const [pending, run] = usePending(onAdopt);
  return (
    <button className={styles.btn} onClick={() => void run()} disabled={pending}>
      {pending && <Spinner size={12} />}
      {t("添加到 CLAUDE.md")}
    </button>
  );
}
