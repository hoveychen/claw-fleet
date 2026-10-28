import { LoaderCircle, Shuffle, Swords } from "lucide-react";
import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  dismissExplanation,
  explainSelection,
  getExplanation,
  listExplanations,
  pollExplanation,
  type ExplainPreset,
  type ExplainRecord,
  type ExplainRequest,
} from "../explainApi";
import {
  cacheHitRatio,
  costLabel,
  flipConditionsQuote,
  groupExplainThreads,
  steelmanQuote,
  type AssistantSelection,
  type ExplainPrime,
} from "../selectionExplain";
import { useSessionsStore } from "../store";
import { TextBlock } from "./blocks/TextBlock";
import { ExplainFollowUp } from "./SessionAuxExplain";
import { SkeletonCard } from "./loading";
import { useDelayedFlag } from "../hooks/useDelayedFlag";
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
  /** The stored records for this card are still being read; `answers` being
   *  empty means "not known yet", not "none asked". */
  loading?: boolean;
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
export function useCardExplain(
  sessionId: string | null | undefined,
  cardTimestamp?: string | null,
): {
  explain: DecisionExplain;
  inline: boolean;
} {
  const panel = useContext(DecisionExplainCtx);
  // Hooks cannot be conditional: the own-state hook always runs, but with a
  // null session when the panel already owns it, which keeps it inert.
  const own = useDecisionExplainMarks(panel ? null : sessionId, cardTimestamp);
  return { explain: panel ?? own, inline: !panel };
}

/**
 * Which stored records belong to the card: asked from a card (no transcript
 * anchor), not dismissed, and created no earlier than the card was posted.
 * Records carry no card id, so the card's posting time is what keeps an older
 * card's questions on the same session from piling onto this one.
 */
export function isCardExplain(rec: ExplainRecord, sinceMs: number): boolean {
  return !rec.anchor && !rec.dismissed && rec.createdMs >= sinceMs;
}

/**
 * `cardTimestamp` is the card request's RFC 3339 `timestamp`. The records are
 * re-read from the store whenever the card changes, so switching to another
 * card and back hands the side questions back instead of an empty panel (they
 * used to live in this hook's state alone, reset on every switch).
 */
export function useDecisionExplainMarks(
  sessionId: string | null | undefined,
  cardTimestamp?: string | null,
): DecisionExplain {
  const session = useSessionsStore((s) => s.sessions.find((x) => x.id === sessionId));
  const sessionPath = session?.jsonlPath;
  const workspacePath = session?.workspacePath;
  const [answers, setAnswers] = useState<ExplainRecord[]>([]);
  const [busy, setBusy] = useState(false);
  const [listing, setListing] = useState(false);
  const pollers = useRef(new Map<string, AbortController>());
  // The session the state belongs to, readable from async callbacks so a
  // reply that lands after a switch cannot seed the next card's answers.
  const current = useRef(sessionId);
  current.current = sessionId;

  const upsert = useCallback((rec: ExplainRecord) => {
    if (rec.sessionId !== current.current) return;
    setAnswers((prev) => {
      const i = prev.findIndex((r) => r.id === rec.id);
      if (i < 0) return [...prev, rec];
      const next = prev.slice();
      next[i] = rec;
      return next;
    });
  }, []);

  const track = useCallback(
    (sid: string, id: string) => {
      if (pollers.current.has(id)) return;
      const ctl = new AbortController();
      pollers.current.set(id, ctl);
      pollExplanation(() => getExplanation(sid, id), upsert, { signal: ctl.signal })
        .catch((e) => console.error("explain poll failed:", e))
        .finally(() => {
          if (pollers.current.get(id) === ctl) pollers.current.delete(id);
        });
    },
    [upsert],
  );

  // A new card starts from what the store holds for it and stops polling the
  // old one's records.
  useEffect(() => {
    setAnswers([]);
    setListing(Boolean(sessionId));
    let alive = true;
    if (sessionId) {
      const parsed = cardTimestamp ? Date.parse(cardTimestamp) : NaN;
      const sinceMs = Number.isFinite(parsed) ? parsed : 0;
      listExplanations(sessionId)
        .then((list) => {
          if (!alive) return;
          const mine = list.filter((r) => isCardExplain(r, sinceMs));
          // Anything asked while the list was in flight is already in state;
          // keep it and add the stored ones around it.
          setAnswers((prev) => {
            const seen = new Set(prev.map((r) => r.id));
            return [...mine.filter((r) => !seen.has(r.id)), ...prev].sort(
              (a, b) => a.createdMs - b.createdMs || a.id.localeCompare(b.id),
            );
          });
          for (const r of mine) if (r.status === "running") track(sessionId, r.id);
        })
        .catch((e) => console.error("list_explanations failed:", e))
        .finally(() => {
          if (alive) setListing(false);
        });
    }
    return () => {
      alive = false;
      for (const ctl of pollers.current.values()) ctl.abort();
      pollers.current.clear();
    };
  }, [sessionId, cardTimestamp, track]);

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
          dismissed: false,
        };
      } finally {
        setBusy(false);
      }
      upsert(rec);
      if (rec.status === "running") track(req.sessionId, rec.id);
    },
    [upsert, track],
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

  // Persisted like the rail's ✕: answers are now re-read from the store on
  // every card switch, so a dismissal kept only in state would come straight
  // back. An ask that never reached the store (`local-` id) has nothing to write.
  const dismiss = useCallback((id: string) => {
    pollers.current.get(id)?.abort();
    pollers.current.delete(id);
    setAnswers((prev) => prev.filter((r) => r.id !== id));
    const sid = current.current;
    if (sid && !id.startsWith("local-")) {
      dismissExplanation(sid, id, true).catch((e) => console.error("dismiss_explanation failed:", e));
    }
  }, []);

  return {
    enabled: Boolean(sessionId && sessionPath),
    busy,
    ask,
    followUp,
    answers,
    loading: listing,
    dismiss,
  };
}

/**
 * The card review asks — "steelman this option" and "what would flip it".
 *
 * A press never sends: it hands the card's `SelectionToolbar` a quote and a
 * preset to preselect (`prime`), and the reader confirms there. `reset`
 * clears it when the card steps to another question.
 */
export function useCardReview(): {
  prime: ExplainPrime | null;
  steelman: (question: string, option: { label: string; description?: string | null }) => void;
  flip: (question: string) => void;
  reset: () => void;
} {
  const [prime, setPrime] = useState<ExplainPrime | null>(null);
  const steelman = useCallback(
    (question: string, option: { label: string; description?: string | null }) =>
      setPrime((p) => ({
        quote: steelmanQuote(question, option),
        preset: "steelman",
        caption: option.label,
        nonce: (p?.nonce ?? 0) + 1,
      })),
    [],
  );
  const flip = useCallback(
    (question: string) =>
      setPrime((p) => ({
        quote: flipConditionsQuote(question),
        preset: "flip_conditions",
        caption: "",
        nonce: (p?.nonce ?? 0) + 1,
      })),
    [],
  );
  const reset = useCallback(() => setPrime(null), []);
  return { prime, steelman, flip, reset };
}

/** The question-level "what would flip it" button, under the question. */
export function FlipConditionsButton({ onPress }: { onPress: () => void }) {
  const { t } = useTranslation();
  return (
    <div className={styles.review_row}>
      <button
        type="button"
        className={styles.review_btn}
        // Keep the mousedown from reaching the bar's outside-press close.
        onMouseDown={(e) => e.preventDefault()}
        onClick={onPress}
        title={t("detail.explain_flip_hint", "什么证据出现时结论会翻转？先在追问浮条里确认再发")}
        data-testid="card-flip-btn"
      >
        <Shuffle size={11} strokeWidth={1.8} aria-hidden="true" />
        {t("detail.explain_flip_btn", "翻转条件")}
      </button>
    </div>
  );
}

/** The per-option "steelman" button, beside the option's pencil. */
export function SteelmanButton({ label, onPress }: { label: string; onPress: () => void }) {
  const { t } = useTranslation();
  const hint = t("detail.explain_steelman_hint", "反方论证：不同意这个选项的专家会怎么说");
  return (
    <button
      type="button"
      className={styles.steelman_btn}
      onMouseDown={(e) => e.preventDefault()}
      onClick={(e) => {
        e.stopPropagation();
        onPress();
      }}
      title={hint}
      aria-label={`${hint} — ${label}`}
      data-testid="card-steelman-btn"
    >
      <Swords size={11} strokeWidth={1.8} aria-hidden="true" />
      {t("detail.explain_steelman_btn", "反方")}
    </button>
  );
}

/**
 * The answers asked from inside a card's question, under it, one block per
 * chain: the quoted text once, then every turn in asking order (its question
 * once the chain has more than one), and — when the latest turn has settled
 * and the caller can fork — a box to keep asking about the same passage. A
 * follow-up is its own record threading off the one it continues, so without
 * the grouping a two-turn exchange would read as two unrelated answers.
 */
export function DecisionExplainAnswers({
  answers,
  loading = false,
  onDismiss,
  onFollowUp,
}: {
  answers: ExplainRecord[];
  /** The stored answers are still being read (see `DecisionExplain.loading`). */
  loading?: boolean;
  onDismiss: (id: string) => void;
  /** Continue a chain from its latest turn. Absent hides the follow-up box. */
  onFollowUp?: (prev: ExplainRecord, question: string) => void | Promise<unknown>;
}) {
  const { t } = useTranslation();
  // Delayed: the read is a local store hit, and most cards have no side
  // questions — a placeholder that flashed on every card switch would be the
  // very jump it exists to prevent.
  const showSkeleton = useDelayedFlag(loading && answers.length === 0);
  if (answers.length === 0) {
    return showSkeleton ? (
      <div className={styles.list}>
        <SkeletonCard height={56} />
      </div>
    ) : null;
  }
  // Oldest chain first, like the answers before grouping: a card's side
  // questions read top-down in the order they were asked.
  const threads = groupExplainThreads(answers).reverse();
  return (
    <div className={styles.list} data-testid="decision-explain-answers">
      {threads.map(({ id, records }) => {
        const first = records[0];
        const last = records[records.length - 1];
        const running = last.status === "running";
        const hit = cacheHitRatio(last);
        const cost = costLabel(last.costUsd);
        const multi = records.length > 1;
        return (
          <div key={id} className={styles.card} data-testid="decision-explain-thread">
            <div className={styles.head}>
              <span className={styles.quote} title={first.question || undefined}>
                {first.quote}
              </span>
              {multi && (
                <span className={styles.state}>
                  {t("detail.explain_turns", "{{count}} 轮", { count: records.length })}
                </span>
              )}
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
                onClick={() => {
                  for (const r of records) onDismiss(r.id);
                }}
                title={t("common.close", "关闭")}
                aria-label={t("common.close", "关闭")}
              >
                ✕
              </button>
            </div>
            {records.map((rec) => (
              <div key={rec.id} className={multi ? styles.turn : undefined}>
                {rec.question && <div className={styles.question}>{rec.question}</div>}
                {rec.text ? (
                  <div className={styles.answer}>
                    <TextBlock text={rec.text} isPartial={rec.status === "running"} />
                  </div>
                ) : rec.status === "running" ? (
                  <div className={styles.waiting}>{t("detail.explain_waiting", "正在 fork 会话作答…")}</div>
                ) : null}
                {rec.status === "error" && (
                  <div className={styles.error}>{rec.error || t("detail.explain_failed", "失败")}</div>
                )}
              </div>
            ))}
            {onFollowUp && last.status === "done" && (
              <div className={styles.follow}>
                <ExplainFollowUp onFollowUp={(q) => onFollowUp(last, q)} />
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
