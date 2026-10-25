import { useTranslation } from "react-i18next";
import { VenetianMask } from "lucide-react";
import styles from "./IncognitoBadge.module.css";

/** Marks an incognito session (Fleet's private window) on a row or a detail
 *  header. `compact` drops the label for tight list rows. */
export function IncognitoBadge({ compact = false }: { compact?: boolean }) {
  const { t } = useTranslation();
  return (
    <span
      className={styles.badge}
      title={t("incognito_badge_title")}
      data-testid="incognito-badge"
    >
      <VenetianMask size={11} strokeWidth={1.9} />
      {!compact && <span>{t("incognito_badge")}</span>}
    </span>
  );
}
