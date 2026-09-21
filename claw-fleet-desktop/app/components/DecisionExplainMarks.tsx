import { LoaderCircle } from "lucide-react";
import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  explainSelection,
  getExplanation,
  pollExplanation,
  type ExplainPreset,
  type ExplainRecord,
  type ExplainRequest,
} from "../explainApi";
import { cacheHitRatio, costLabel, type AssistantSelection } from "../selectionExplain";
import { useSessionsStore } from "../store";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./DecisionExplainMarks.module.css";

/**
 * Side questions asked from inside a decision card's question.
 *
 * The card's question body is stamped `data-role="assistant"` /
 * `data-msg-idx` (the question index) so the same `SelectionToolbar` that
 * floats over transcript prose floats over it: a drag, or a click on one of the
 * agent's `[?text]` marks, selects a passage and the bar offers the presets.
 * The ask goes to the card's session with an empty anchor (a card is not a
 * transcript row) and the answer lands *under the question*, inside the card —
 * the reader is deciding something and should not have to open the session's
 * rail to read a clarification. The record is persisted like every side
 * question, so the rail shows it later too.
 *
 * `sessionPath` (the fork target) is not on the request; it is read off the
 * sessions store. A card whose session the store does not know yet has
 * `enabled: false`, which keeps the bar away.
 */
export type DecisionExplain = {
  enabled: boolean;
  busy: boolean;
  ask: (sel: AssistantSelection, preset: ExplainPreset, question?: string) => void;
  /**
   * Continue a settled answer: same passage, the prior Q/A folded into a fresh
   * fork of the *session* (the fork itself is never resumable — see
   * `session_explain::build_prompt`). Threading it off `prev` is what keeps the
   * follow-up aware of the answer it is following up on.
   */
  followUp: (prev: ExplainRecord, question: string) => void;
  answers: ExplainRecord[];
  dismiss: (id: string) => void;
};

/**
 * The panel-level side-question state, when there is one.
 *
 * `DecisionPanel` owns the state so the answers can be rendered in its side
 * column instead of at the tail of the question body, where they were sharing
 * a scroller with the prose and got squeezed into an unreadable sliver under a
 * long card (the footer is flex-none and wins the height). A card rendered
 * outside the panel — `SessionDetail`'s inline compact card — sees `null` here
 * and falls back to owning the state itself, answers under the question.
 */
const DecisionExplainCtx = createContext<DecisionExplain | null>(null);

export const DecisionExplainProvider = DecisionExplainCtx.Provider;

/**
 * What a card should use: the panel's state when the card sits in the panel,
 * its own otherwise. `inline` says whether the card has to render the answers
 * itself (nothing else will).
 */
export function useCardExplain(sessionId: string | null | undefined): {
  explain: DecisionExplain;
  inline: boolean;
} {
  const panel = useContext(DecisionExplainCtx);
  // Hooks cannot be conditional: the own-state hook always runs, but with a
  // null session when the panel already owns it, which keeps it inert.
  const own = useDecisionExplainMarks(panel ? null : sessionId);
  return { explain: panel ?? own, inline: !panel };
}

export function useDecisionExplainMarks(sessionId: string | null | undefined): DecisionExplain {
  const session = useSessionsStore((s) => s.sessions.find((x) => x.id === sessionId));
  const sessionPath = session?.jsonlPath;
  const workspacePath = session?.workspacePath;
  const [answers, setAnswers] = useState<ExplainRecord[]>([]);
  const [busy, setBusy] = useState(false);
  const pollers = useRef(new Map<string, AbortController>());

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

  /** Post a request, show the record (or a local error stand-in), and follow it
   *  until it settles. Shared by the first question and every follow-up. */
  const submit = useCallback(
    async (req: ExplainRequest) => {
      setBusy(true);
      let rec: ExplainRecord;
      try {
        rec = await explainSelection(req);
      } catch (e) {
        const now = Date.now();
        rec = {
          id: `local-${now}`,
          sessionId: req.sessionId,
          source: "",
          createdMs: now,
          updatedMs: now,
          preset: req.preset,
          quote: req.quote,
          question: req.question ?? "",
          anchor: req.anchor,
          thread: req.thread ?? [],
          status: "error",
          text: "",
          error: e instanceof Error ? e.message : String(e),
          inputTokens: 0,
          outputTokens: 0,
          cacheReadTokens: 0,
          cacheCreationTokens: 0,
          durationMs: 0,
        };
      } finally {
        setBusy(false);
      }
      upsert(rec);
      if (rec.status === "running" && !pollers.current.has(rec.id)) {
        const ctl = new AbortController();
        pollers.current.set(rec.id, ctl);
        pollExplanation(() => getExplanation(req.sessionId, rec.id), upsert, { signal: ctl.signal })
          .catch((e) => console.error("explain poll failed:", e))
          .finally(() => {
            if (pollers.current.get(rec.id) === ctl) pollers.current.delete(rec.id);
          });
      }
    },
    [upsert],
  );

  const ask = useCallback(
    async (sel: AssistantSelection, preset: ExplainPreset, question?: string) => {
      if (!sessionId || !sessionPath) return;
      await submit({
        sessionId,
        sessionPath,
        workspacePath: workspacePath || undefined,
        quote: sel.quote,
        preset,
        question: preset === "custom" ? question : undefined,
        anchor: undefined,
        thread: [],
      });
      window.getSelection()?.removeAllRanges();
    },
    [sessionId, sessionPath, workspacePath, submit],
  );

  const followUp = useCallback(
    async (prev: ExplainRecord, question: string) => {
      if (!sessionId || !sessionPath) return;
      await submit({
        sessionId,
        sessionPath,
        workspacePath: workspacePath || undefined,
        quote: prev.quote,
        preset: "custom",
        question,
        anchor: prev.anchor,
        thread: [...(prev.thread ?? []), prev.id],
      });
    },
    [sessionId, sessionPath, workspacePath, submit],
  );

  const dismiss = useCallback((id: string) => {
    pollers.current.get(id)?.abort();
    pollers.current.delete(id);
    setAnswers((prev) => prev.filter((r) => r.id !== id));
  }, []);

  return { enabled: Boolean(sessionId && sessionPath), busy, ask, followUp, answers, dismiss };
}

/** The answers asked from inside a card's question, under it: the quoted
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
              <span className={styles.quote} title={rec.question || undefined}>
                {rec.quote}
              </span>
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
            {rec.question && <div className={styles.question}>{rec.question}</div>}
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
