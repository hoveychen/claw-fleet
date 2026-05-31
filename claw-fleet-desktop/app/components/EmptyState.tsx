import { useTranslation } from "react-i18next";
import { MessagesSquare, Loader2 } from "lucide-react";
import styles from "./EmptyState.module.css";

interface EmptyStateProps {
  /**
   * Whether the initial background scan has completed. While `false` (and no
   * sessions exist yet) we show a lightweight "scanning…" state instead of the
   * onboarding hint, so we don't flash "no sessions" before the first scan
   * resolves.
   */
  scanReady: boolean;
  /**
   * Whether *any* sessions exist at all (regardless of the current filter).
   * When true, this empty region is just a filtered view with no matches — we
   * show a neutral "no matching sessions" line, NOT the onboarding hint, since
   * the user clearly already has sessions.
   */
  hasSessions: boolean;
}

/**
 * Friendly empty state for the session list / gallery. Three cases:
 *  - scan not done yet AND no sessions at all → subtle scanning spinner;
 *  - scan done AND no sessions at all → onboarding hint guiding the user to
 *    start an agent session (Fleet only observes externally-launched sessions);
 *  - sessions exist but none match the current filter → neutral "no match".
 */
export function EmptyState({ scanReady, hasSessions }: EmptyStateProps) {
  const { t } = useTranslation();

  if (hasSessions) {
    return (
      <div className={styles.wrap}>
        <p className={styles.scanning}>{t("empty_state.no_match")}</p>
      </div>
    );
  }

  if (!scanReady) {
    return (
      <div className={styles.wrap}>
        <Loader2 className={styles.spinner} size={20} strokeWidth={1.75} />
        <p className={styles.scanning}>{t("empty_state.scanning")}</p>
      </div>
    );
  }

  return (
    <div className={styles.wrap}>
      <div className={styles.icon}>
        <MessagesSquare size={28} strokeWidth={1.5} />
      </div>
      <p className={styles.title}>{t("empty_state.title")}</p>
      <p className={styles.subtitle}>{t("empty_state.subtitle")}</p>
    </div>
  );
}
