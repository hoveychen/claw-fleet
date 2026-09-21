import { LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  explainSelection,
  getExplanation,
  pollExplanation,
  type ExplainRecord,
  type ExplainRequest,
} from "../explainApi";
import type { ExplainMarksContext } from "../markdown/explainMarks";
import { cacheHitRatio, costLabel } from "../selectionExplain";
import { useSessionsStore } from "../store";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./DecisionExplainMarks.module.css";

/**
 * `[?text]` marks inside a decision card's question.
 *
 * A card is not a transcript row, so a mark there has no `data-msg-idx` to
 * anchor to; the ask goes to the card's session with an empty anchor and the
 * answer lands *under the question*, inside the card — the reader is deciding
 * something and should not have to open the session's rail to read a
 * clarification. The record is persisted like every side question, so the
 * rail shows it later too.
 *
 * `sessionPath` (the fork target) is not on the request; it is read off the
 * sessions store. A card whose session the store does not know yet gets a
 * `null` context, which renders the marks as plain text.
 */
export function useDecisionExplainMarks(sessionId: string | null | undefined): {
  marks: ExplainMarksContext | null;
  answers: ExplainRecord[];
  dismiss: (id: string) => void;
} {
  const session = useSessionsStore((s) => s.sessions.find((x) => x.id === sessionId));
  const sessionPath = session?.jsonlPath;
  const workspacePath = session?.workspacePath;
  const [answers, setAnswers] = useState<ExplainRecord[]>([]);
  const pollers = useRef(new Map<string, AbortController>());
  const answersRef = useRef(answers);
  answersRef.current = answers;

  const upsert = useCallback((rec: ExplainRecord) => {
    setAnswers((prev) => {
      const i = prev.findIndex((r) => r.id === rec.id);
      if (i < 0) return [...prev, rec];
      const next = prev.slice();
      next[i] = rec;
      return next;
    });
  }, []);

  // A new session (a new card) starts clean and stops polling the old one.
  useEffect(() => {
    return () => {
      for (const ctl of pollers.current.values()) ctl.abort();
      pollers.current.clear();
      setAnswers([]);
    };
  }, [sessionId]);

  const onMark = useCallback(
    async (quote: string) => {
      if (!sessionId || !sessionPath) return;
      // Already asked (and not failed): the answer is on screen below.
      if (answersRef.current.some((r) => r.quote === quote && r.status !== "error")) return;
      const req: ExplainRequest = {
        sessionId,
        sessionPath,
        workspacePath: workspacePath || undefined,
        quote,
        preset: "explain",
        anchor: undefined,
        thread: [],
      };
      let rec: ExplainRecord;
      try {
        rec = await explainSelection(req);
      } catch (e) {
        const now = Date.now();
        rec = {
          id: `local-${now}`,
          sessionId,
          source: "",
          createdMs: now,
          updatedMs: now,
          preset: "explain",
          quote,
          question: "",
          anchor: undefined,
          thread: [],
          status: "error",
          text: "",
          error: e instanceof Error ? e.message : String(e),
          inputTokens: 0,
          outputTokens: 0,
          cacheReadTokens: 0,
          cacheCreationTokens: 0,
          durationMs: 0,
        };
      }
      upsert(rec);
      if (rec.status === "running" && !pollers.current.has(rec.id)) {
        const ctl = new AbortController();
        pollers.current.set(rec.id, ctl);
        pollExplanation(() => getExplanation(sessionId, rec.id), upsert, { signal: ctl.signal })
          .catch((e) => console.error("explain poll failed:", e))
          .finally(() => {
            if (pollers.current.get(rec.id) === ctl) pollers.current.delete(rec.id);
          });
      }
    },
    [sessionId, sessionPath, workspacePath, upsert],
  );

  const dismiss = useCallback((id: string) => {
    pollers.current.get(id)?.abort();
    pollers.current.delete(id);
    setAnswers((prev) => prev.filter((r) => r.id !== id));
  }, []);

  const marks = useMemo<ExplainMarksContext | null>(
    () => (sessionId && sessionPath ? { onMark } : null),
    [sessionId, sessionPath, onMark],
  );
  return { marks, answers, dismiss };
}

/** The answers asked from a card's marks, under its question: the quoted
 *  text, then the answer growing as the record is re-read. */
export function DecisionExplainAnswers({
  answers,
  onDismiss,
}: {
  answers: ExplainRecord[];
  onDismiss: (id: string) => void;
}) {
  const { t } = useTranslation();
  if (answers.length === 0) return null;
  return (
    <div className={styles.list} data-testid="decision-explain-answers">
      {answers.map((rec) => {
        const running = rec.status === "running";
        const failed = rec.status === "error";
        const hit = cacheHitRatio(rec);
        const cost = costLabel(rec.costUsd);
        return (
          <div key={rec.id} className={styles.card}>
            <div className={styles.head}>
              <span className={styles.quote}>{rec.quote}</span>
              {running && (
                <span className={styles.state}>
                  <LoaderCircle size={10} aria-hidden="true" className={styles.spin} />
                  {t("detail.explain_running", "追问中")}
                </span>
              )}
              {!running && cost && <span className={styles.state}>{cost}</span>}
              {!running && hit != null && (
                <span className={styles.state}>
                  {t("detail.explain_cache_hit_short", "缓存")} {Math.round(hit * 100)}%
                </span>
              )}
              <button
                type="button"
                className={styles.close}
                onClick={() => onDismiss(rec.id)}
                title={t("common.close", "关闭")}
                aria-label={t("common.close", "关闭")}
              >
                ✕
              </button>
            </div>
            {rec.text ? (
              <div className={styles.answer}>
                <TextBlock text={rec.text} isPartial={running} />
              </div>
            ) : running ? (
              <div className={styles.waiting}>{t("detail.explain_waiting", "正在 fork 会话作答…")}</div>
            ) : null}
            {failed && <div className={styles.error}>{rec.error || t("detail.explain_failed", "失败")}</div>}
          </div>
        );
      })}
    </div>
  );
}
