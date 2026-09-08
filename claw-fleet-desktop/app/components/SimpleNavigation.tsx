import { useTranslation } from "react-i18next";
import { Settings } from "lucide-react";
import { useUIStore } from "../store";
import styles from "./SimpleNavigation.module.css";

export function SimpleNavigation() {
  const { t } = useTranslation();
  const { viewMode, setViewMode, setSettingsOpen } = useUIStore();
  return (
    <header className={styles.header} data-tauri-drag-region>
      <nav className={styles.tabs} aria-label={t("simple_navigation")}>
        {(["history", "artifacts"] as const).map((view) => (
          <button key={view} type="button" aria-current={viewMode === view ? "page" : undefined}
            className={styles.tab} onClick={() => setViewMode(view)}>
            {t(view === "history" ? "view_history" : "view_artifacts")}
          </button>
        ))}
      </nav>
      <button type="button" className={styles.settings} aria-label={t("settings.title")}
        title={t("settings.title")} onClick={() => setSettingsOpen(true)}>
        <Settings size={18} />
      </button>
    </header>
  );
}
