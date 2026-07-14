import { useTranslation } from "react-i18next";
import { useUIStore } from "../store";
import styles from "./SkillsSourceTabs.module.css";

/**
 * Segmented switch that merges the former standalone 技能 / 插件 nav entries into
 * one page. Plugins are a source of skills, so both now live under the 技能 nav
 * item and toggle here.
 *
 * We deliberately keep two distinct ViewModes ("skills" / "plugins"): each page
 * owns its own rail width + secondary sidebar via PageShell, and flipping the
 * ViewMode preserves all of that. This control only switches between them.
 */
export function SkillsSourceTabs() {
  const { t } = useTranslation();
  const viewMode = useUIStore((s) => s.viewMode);
  const setViewMode = useUIStore((s) => s.setViewMode);
  return (
    <div className={styles.tab_bar}>
      <button
        type="button"
        className={`${styles.tab_btn} ${viewMode === "skills" ? styles.tab_active : ""}`}
        onClick={() => setViewMode("skills")}
      >
        {t("view_skills")}
      </button>
      <button
        type="button"
        className={`${styles.tab_btn} ${viewMode === "plugins" ? styles.tab_active : ""}`}
        onClick={() => setViewMode("plugins")}
      >
        {t("view_plugins")}
      </button>
    </div>
  );
}
