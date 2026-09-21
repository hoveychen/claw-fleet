import { useTranslation } from "react-i18next";

import { DecisionExplainAnswers, type DecisionExplain } from "./DecisionExplainMarks";
import styles from "./DecisionExplainColumn.module.css";

/**
 * The side questions asked from inside the active card, in the panel's side
 * column.
 *
 * They used to render at the tail of the question body, inside the same
 * scroller as the prose. On a long card with a tall footer that left them a
 * clipped sliver the reader never scrolled to — the ask looked like it had
 * done nothing at all. Here they get their own scroller and their own height,
 * and the card's prose and options keep theirs.
 */
export function DecisionExplainColumn({ explain }: { explain: DecisionExplain }) {
  const { t } = useTranslation();
  return (
    <div className={styles.column} data-testid="decision-explain-column">
      <div className={styles.head}>
        {t("decision_panel.explain_column", "追问")}
        <span className={styles.count}>{explain.answers.length}</span>
      </div>
      <div className={styles.body}>
        <DecisionExplainAnswers answers={explain.answers} onDismiss={explain.dismiss} />
      </div>
    </div>
  );
}
