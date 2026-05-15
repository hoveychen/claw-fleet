import { useTranslation } from "react-i18next";
import { useUIStore } from "../store";
import styles from "./SessionToolbar.module.css";

interface SessionToolbarProps {
  filter: string;
  onFilterChange: (value: string) => void;
  activeCount: number;
  totalCount: number;
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
  totalCount,
  showAll,
  onToggleShowAll,
  ftsMatchCount,
  searching,
}: SessionToolbarProps) {
  const { t } = useTranslation();
  const viewMode = useUIStore((s) => s.viewMode);
  const setViewMode = useUIStore((s) => s.setViewMode);

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
      <span className={styles.total_badge} title={`${activeCount} active`}>
        {totalCount}
      </span>
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
      <div className={styles.view_toggle} role="group" aria-label={t("view_sessions")}>
        <button
          type="button"
          className={`${styles.view_toggle_btn} ${viewMode === "list" ? styles.view_toggle_btn_active : ""}`}
          onClick={() => setViewMode("list")}
          title={t("view_mode_list_tooltip")}
          aria-label={t("view_mode_list_tooltip")}
          aria-pressed={viewMode === "list"}
        >
          ☰
        </button>
        <button
          type="button"
          className={`${styles.view_toggle_btn} ${viewMode === "gallery" ? styles.view_toggle_btn_active : ""}`}
          onClick={() => setViewMode("gallery")}
          title={t("view_mode_gallery_tooltip")}
          aria-label={t("view_mode_gallery_tooltip")}
          aria-pressed={viewMode === "gallery"}
        >
          ⊞
        </button>
      </div>
    </div>
  );
}
