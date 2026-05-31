import { useTranslation } from "react-i18next";
import { MessagesSquare, Loader2 } from "lucide-react";
import type { ReactNode } from "react";
import styles from "./EmptyState.module.css";

interface EmptyStateAction {
  label: string;
  onClick: () => void;
}

interface EmptyStateProps {
  /** Friendly illustration — usually a lucide icon sized ~28px. */
  icon: ReactNode;
  /** Short headline. */
  title: string;
  /** Optional supporting line under the title. */
  subtitle?: string;
  /** Optional call-to-action button shown under the text. */
  action?: EmptyStateAction;
}

/**
 * Shared friendly empty state: a framed icon + title + optional subtitle +
 * optional CTA button. Use this for any "no data yet" region so every view
 * shares one look. For the session list / gallery's three-state behaviour
 * (scanning / no-match / onboarding) use {@link SessionEmptyState} instead.
 */
export function EmptyState({ icon, title, subtitle, action }: EmptyStateProps) {
  return (
    <div className={styles.wrap}>
      <div className={styles.icon}>{icon}</div>
      <p className={styles.title}>{title}</p>
      {subtitle && <p className={styles.subtitle}>{subtitle}</p>}
      {action && (
        <button type="button" className={styles.cta} onClick={action.onClick}>
          {action.label}
        </button>
      )}
    </div>
  );
}

interface SessionEmptyStateProps {
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
 * Session list / gallery empty state. Three cases:
 *  - sessions exist but none match the current filter → neutral "no match";
 *  - scan not done yet AND no sessions at all → subtle scanning spinner;
 *  - scan done AND no sessions at all → friendly onboarding hint guiding the
 *    user to start an agent session (Fleet only observes external sessions).
 */
export function SessionEmptyState({ scanReady, hasSessions }: SessionEmptyStateProps) {
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
    <EmptyState
      icon={<MessagesSquare size={28} strokeWidth={1.5} />}
      title={t("empty_state.title")}
      subtitle={t("empty_state.subtitle")}
    />
  );
}
