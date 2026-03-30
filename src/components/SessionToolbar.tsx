import { useTranslation } from "react-i18next";
import styles from "./SessionToolbar.module.css";

interface SessionToolbarProps {
  filter: string;
  onFilterChange: (value: string) => void;
  activeCount: number;
  showAll: boolean;
  onToggleShowAll: () => void;
  /** Number of extra sessions matched by full-text search (beyond client-side filter). */
  ftsMatchCount?: number;
  /** Whether a full-text search is in progress. */
  searching?: boolean;
}

export function SessionToolbar({
  filter,
  onFilterChange,
  activeCount,
  showAll,
  onToggleShowAll,
  ftsMatchCount,
  searching,
}: SessionToolbarProps) {
  const { t } = useTranslation();

  return (
    <div className={styles.toolbar} data-tauri-drag-region>
      <div className={styles.search_wrap}>
        <input
          className={styles.search}
          type="text"
          placeholder={t("filter_placeholder")}
          value={filter}
          onChange={(e) => onFilterChange(e.target.value)}
        />
        {searching && <span className={styles.spinner} />}
      </div>
      <span className={styles.count}>
        {activeCount} {t("active")}
        {ftsMatchCount != null && ftsMatchCount > 0 && (
          <span className={styles.fts_count}>
            {" "}+ {ftsMatchCount} {t("search_matches", "matched")}
          </span>
        )}
      </span>
      <button
        className={`${styles.toggle_btn} ${showAll ? styles.toggle_btn_active : ""}`}
        onClick={onToggleShowAll}
        title={showAll ? t("gallery_show_active") : t("gallery_show_all")}
      >
        {showAll ? t("gallery_show_active") : t("gallery_show_all")}
      </button>
    </div>
  );
}
