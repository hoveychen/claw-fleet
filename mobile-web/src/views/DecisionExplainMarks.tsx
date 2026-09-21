// `[?text]` marks inside a decision card's question, phone side — the
// counterpart of claw-fleet-desktop/app/components/DecisionExplainMarks.tsx.
//
// A card is not a transcript row, so a mark there has no row to anchor to; the
// ask goes to the card's session with an empty anchor and the answer lands
// under the question, inside the card, where the person deciding can read it
// without leaving. The record is persisted like every side question, so the
// session's 追问 pane shows it later too.
import { LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { cacheHitRatio, costLabel, pollExplanation } from "../../../shared-ts/sessionExplain";
import { t } from "../i18n";
import type { ExplainMarksContext } from "../markdown/explainMarks";
import {
  askExplanation,
  getExplanation,
  refusedExplanation,
  type ExplainRecord,
  type ExplainRequest,
} from "../sessionExplain";
import type { FleetTransport } from "../transport";
import type { SessionInfo } from "../types";
import { Md } from "./DecisionQa";
import styles from "./DecisionExplainMarks.module.css";

export function useDecisionExplainMarks(
  client: FleetTransport | null,
  session: SessionInfo | undefined,
): {
  marks: ExplainMarksContext | null;
  answers: ExplainRecord[];
  dismiss: (id: string) => void;
} {
  const sessionId = session?.id;
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

  useEffect(() => {
    return () => {
      for (const ctl of pollers.current.values()) ctl.abort();
      pollers.current.clear();
      setAnswers([]);
    };
  }, [sessionId]);

  const onMark = useCallback(
    async (quote: string) => {
      if (!client || !sessionId || !sessionPath) return;
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
        rec = await askExplanation(client, req);
      } catch (e) {
        rec = refusedExplanation(req, e);
      }
      upsert(rec);
      if (rec.status === "running" && !pollers.current.has(rec.id)) {
        const ctl = new AbortController();
        pollers.current.set(rec.id, ctl);
        pollExplanation(() => getExplanation(client, sessionId, rec.id), upsert, { signal: ctl.signal })
          .catch((e) => console.error("explain poll failed:", e))
          .finally(() => {
            if (pollers.current.get(rec.id) === ctl) pollers.current.delete(rec.id);
          });
      }
    },
    [client, sessionId, sessionPath, workspacePath, upsert],
  );

  const dismiss = useCallback((id: string) => {
    pollers.current.get(id)?.abort();
    pollers.current.delete(id);
    setAnswers((prev) => prev.filter((r) => r.id !== id));
  }, []);

  const marks = useMemo<ExplainMarksContext | null>(
    () => (client && sessionId && sessionPath ? { onMark } : null),
    [client, sessionId, sessionPath, onMark],
  );
  return { marks, answers, dismiss };
}

/** The answers asked from a card's marks, under its question. */
export function DecisionExplainAnswers({
  answers,
  onDismiss,
}: {
  answers: ExplainRecord[];
  onDismiss: (id: string) => void;
}) {
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
                <span className={styles.state} data-tone="live">
                  <LoaderCircle size={11} aria-hidden="true" className={styles.spin} />
                  {t("追问中")}
                </span>
              )}
              {!running && cost && <span className={styles.state}>{cost}</span>}
              {!running && hit != null && <span className={styles.state}>{t("缓存 {0}%", Math.round(hit * 100))}</span>}
              <button type="button" className={styles.close} onClick={() => onDismiss(rec.id)} aria-label={t("关闭")}>
                ✕
              </button>
            </div>
            {rec.text ? (
              <div className={styles.answer} data-partial={running || undefined}>
                <Md text={rec.text} />
              </div>
            ) : running ? (
              <div className={styles.waiting}>{t("正在 fork 会话作答…")}</div>
            ) : null}
            {failed && <div className={styles.error}>{rec.error || t("失败")}</div>}
          </div>
        );
      })}
    </div>
  );
}
