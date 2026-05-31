import { useTranslation } from "react-i18next";
import { MessagesSquare, Loader2 } from "lucide-react";
import styles from "./EmptyState.module.css";

interface EmptyStateProps {
  /**
   * Whether the initial background scan has completed. While `false` we show a
   * lightweight "scanning…" state instead of the onboarding hint, so we don't
   * flash "no sessions" before the first scan resolves.
   */
  scanReady: boolean;
}

/**
 * Friendly empty state for the session list / gallery. Before the first scan
 * finishes it shows a subtle spinner; once the scan is done and the list is
 * still empty it guides the user to start an agent session (Fleet only
 * observes externally-launched Claude Code sessions).
 */
export function EmptyState({ scanReady }: EmptyStateProps) {
  const { t } = useTranslation();

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
