import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Menu, LayoutGrid, Coffee } from "lucide-react";
import { useKeepAwake } from "../hooks/useKeepAwake";
import { useUIStore } from "../store";
import { PageShell } from "./PageShell";
import styles from "./SessionsBanner.module.css";

interface SessionsPageProps {
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
  /** The list or the gallery grid — whichever view mode is active. */
  children: ReactNode;
}

/**
 * The sessions page: the app shell wearing the list/gallery banner.
 *
 * List and gallery are two renderings of the same page, so they share one banner
 * — which is why this wraps PageShell rather than just returning banner nodes.
 * The page has no secondary sidebar (Boss's call): banner over the main
 * container, nothing else.
 */
export function SessionsPage({
  filter,
  onFilterChange,
  activeCount,
  totalCount,
  showAll,
  onToggleShowAll,
  ftsMatchCount,
  searching,
  children,
}: SessionsPageProps) {
  const { t } = useTranslation();
  const viewMode = useUIStore((s) => s.viewMode);
  const setViewMode = useUIStore((s) => s.setViewMode);
  const { enabled: keepAwake, supported: keepAwakeSupported, setKeepAwake } = useKeepAwake();

  return (
    <PageShell
      view={viewMode}
      title={t("view_sessions")}
      count={totalCount}
      search={{
        value: filter,
        onChange: onFilterChange,
        placeholder: t("filter_placeholder"),
        busy: searching,
      }}
      bannerCenter={
        <span className={styles.count}>
          {activeCount} {t("active")}
          {ftsMatchCount != null && ftsMatchCount > 0 && (
            <span className={styles.fts_count}>
              {" "}+ {ftsMatchCount} {t("search_matches", "matched")}
            </span>
          )}
        </span>
      }
      actions={
        <>
          <button
            className={`${styles.toggle_btn} ${showAll ? styles.toggle_btn_active : ""}`}
            onClick={onToggleShowAll}
            title={showAll ? t("gallery_show_active") : t("gallery_show_all")}
          >
            {showAll ? t("gallery_show_active") : t("gallery_show_all")}
          </button>
          {keepAwakeSupported && (
            <button
              type="button"
              className={`${styles.icon_btn} ${keepAwake ? styles.icon_btn_active : ""}`}
              onClick={() => setKeepAwake(!keepAwake)}
              title={keepAwake ? t("keep_awake_on_tooltip") : t("keep_awake_off_tooltip")}
              aria-label={keepAwake ? t("keep_awake_on_tooltip") : t("keep_awake_off_tooltip")}
              aria-pressed={keepAwake}
            >
              <Coffee size={14} strokeWidth={1.5} />
            </button>
          )}
          <div className={styles.view_toggle} role="group" aria-label={t("view_sessions")}>
            <button
              type="button"
              className={`${styles.view_toggle_btn} ${viewMode === "list" ? styles.view_toggle_btn_active : ""}`}
              onClick={() => setViewMode("list")}
              title={t("view_mode_list_tooltip")}
              aria-label={t("view_mode_list_tooltip")}
              aria-pressed={viewMode === "list"}
            >
              <Menu size={14} strokeWidth={1.5} />
            </button>
            <button
              type="button"
              className={`${styles.view_toggle_btn} ${viewMode === "gallery" ? styles.view_toggle_btn_active : ""}`}
              onClick={() => setViewMode("gallery")}
              title={t("view_mode_gallery_tooltip")}
              aria-label={t("view_mode_gallery_tooltip")}
              aria-pressed={viewMode === "gallery"}
            >
              <LayoutGrid size={14} strokeWidth={1.5} />
            </button>
          </div>
        </>
      }
    >
      {children}
    </PageShell>
  );
}
