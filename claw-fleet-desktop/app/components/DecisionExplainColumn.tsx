import { LoaderCircle, Quote } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import type { ExplainRecord } from "../explainApi";
import { groupExplainThreads } from "../explainThreads";
import { cacheHitRatio, costLabel } from "../selectionExplain";
import type { DecisionExplain } from "./DecisionExplainMarks";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./DecisionExplainColumn.module.css";

/**
 * The side questions asked from inside the active card, in the panel's side
 * column.
 *
 * They used to render at the tail of the question body, inside the same
 * scroller as the prose. On a long card with a tall footer that left them a
 * clipped sliver the reader never scrolled to — the ask looked like it had
 * done nothing at all. Here they get their own scroller and their own height.
 *
 * The typography is this column's own, not `DecisionExplainAnswers`'s: that
 * one is a 11px strip squeezed under a question and has to stay small, while
 * here the answer *is* what the reader came for — quote and question set the
 * context above it, the spend line sits out of the way underneath.
 *
 * Chains, not records: a follow-up is a separate record that threads off the
 * one it continues, so rendered flat and newest-first a two-turn chain read
 * backwards and anything asked in between split it in half. Chains stay
 * newest-first (a follow-up is usually about the last thing asked) but their
 * turns run oldest-first under one shared quote.
 */
export function DecisionExplainColumn({ explain }: { explain: DecisionExplain }) {
  const { t } = useTranslation();
  const threads = groupExplainThreads(explain.answers);
  return (
    <div className={styles.column} data-testid="decision-explain-column">
      <div className={styles.head}>
        <span className={styles.head_title}>{t("decision_panel.explain_column", "追问")}</span>
        <span className={styles.count}>{explain.answers.length}</span>
      </div>
      <div className={styles.body} data-testid="decision-explain-answers">
        {threads.map((thread) => {
          const last = thread.records[thread.records.length - 1];
          return (
            <section key={thread.id} className={styles.thread}>
              <button
                type="button"
                className={styles.close}
                onClick={() => {
                  for (const rec of thread.records) explain.dismiss(rec.id);
                }}
                title={t("common.close", "关闭")}
                aria-label={t("common.close", "关闭")}
              >
                ✕
              </button>
              <blockquote className={styles.quote}>
                <Quote size={11} strokeWidth={2} aria-hidden="true" className={styles.quote_icon} />
                {thread.records[0].quote}
              </blockquote>
              {thread.records.map((rec) => (
                <ExplainTurn key={rec.id} rec={rec} />
              ))}
              <ExplainFollowUp
                busy={explain.busy}
                canAsk={explain.enabled && last.status === "done"}
                onSubmit={(question) => explain.followUp(last, question)}
              />
            </section>
          );
        })}
      </div>
    </div>
  );
}

/** One turn of a chain: the question asked, then the answer as it arrives. */
function ExplainTurn({ rec }: { rec: ExplainRecord }) {
  const { t } = useTranslation();
  const running = rec.status === "running";
  const hit = cacheHitRatio(rec);
  const cost = costLabel(rec.costUsd);
  const seconds = rec.durationMs > 0 ? `${Math.round(rec.durationMs / 1000)}s` : null;
  return (
    <article className={styles.entry}>
      {rec.question && <p className={styles.question}>{rec.question}</p>}
      {rec.text ? (
        <div className={styles.answer}>
          <TextBlock text={rec.text} isPartial={running} />
        </div>
      ) : running ? (
        <p className={styles.waiting}>
          <LoaderCircle size={12} aria-hidden="true" className={styles.spin} />
          {t("detail.explain_waiting", "正在 fork 会话作答…")}
        </p>
      ) : null}
      {rec.status === "error" && (
        <p className={styles.error}>{rec.error || t("detail.explain_failed", "失败")}</p>
      )}
      {!running && (cost || hit != null || seconds) && (
        <p className={styles.meta}>
          {[
            cost,
            hit != null ? `${t("detail.explain_cache_hit_short", "缓存")} ${Math.round(hit * 100)}%` : null,
            seconds,
          ]
            .filter(Boolean)
            .join(" · ")}
        </p>
      )}
    </article>
  );
}

/**
 * Keep asking about the same passage. Only offered once the chain's last turn
 * is `done`: a follow-up folds the prior answers into its prompt, so asking
 * before one has settled would thread off a half-written answer.
 */
function ExplainFollowUp({
  busy,
  canAsk,
  onSubmit,
}: {
  busy: boolean;
  canAsk: boolean;
  onSubmit: (question: string) => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");
  if (!canAsk) return null;
  return (
    <form
      className={styles.follow}
      onSubmit={(e) => {
        e.preventDefault();
        const q = draft.trim();
        if (!q || busy) return;
        onSubmit(q);
        setDraft("");
      }}
    >
      <input
        className={styles.follow_input}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        placeholder={t("detail.explain_follow_up_placeholder", "继续追问这段话…")}
        aria-label={t("detail.explain_follow_up", "继续追问")}
      />
      <button type="submit" className={styles.follow_send} disabled={busy || !draft.trim()}>
        {t("detail.explain_send", "发送")}
      </button>
    </form>
  );
}
