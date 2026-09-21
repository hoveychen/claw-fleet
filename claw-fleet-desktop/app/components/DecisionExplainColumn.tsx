import { LoaderCircle, Quote } from "lucide-react";
import { useTranslation } from "react-i18next";

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
 * context above it, the spend line sits out of the way underneath. Newest
 * first, since a follow-up is usually about the last thing asked.
 */
export function DecisionExplainColumn({ explain }: { explain: DecisionExplain }) {
  const { t } = useTranslation();
  const newestFirst = [...explain.answers].reverse();
  return (
    <div className={styles.column} data-testid="decision-explain-column">
      <div className={styles.head}>
        <span className={styles.head_title}>{t("decision_panel.explain_column", "追问")}</span>
        <span className={styles.count}>{explain.answers.length}</span>
      </div>
      <div className={styles.body} data-testid="decision-explain-answers">
        {newestFirst.map((rec) => {
          const running = rec.status === "running";
          const hit = cacheHitRatio(rec);
          const cost = costLabel(rec.costUsd);
          const seconds = rec.durationMs > 0 ? `${Math.round(rec.durationMs / 1000)}s` : null;
          return (
            <article key={rec.id} className={styles.entry}>
              <button
                type="button"
                className={styles.close}
                onClick={() => explain.dismiss(rec.id)}
                title={t("common.close", "关闭")}
                aria-label={t("common.close", "关闭")}
              >
                ✕
              </button>
              <blockquote className={styles.quote}>
                <Quote size={11} strokeWidth={2} aria-hidden="true" className={styles.quote_icon} />
                {rec.quote}
              </blockquote>
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
                    hit != null
                      ? `${t("detail.explain_cache_hit_short", "缓存")} ${Math.round(hit * 100)}%`
                      : null,
                    seconds,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </p>
              )}
            </article>
          );
        })}
      </div>
    </div>
  );
}
