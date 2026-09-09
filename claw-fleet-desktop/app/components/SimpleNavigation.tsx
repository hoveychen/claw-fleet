import { useTranslation } from "react-i18next";
import { Settings } from "lucide-react";
import { useDecisionStore, useUIStore } from "../store";
import { oldestPendingDecision } from "../pendingDecisionState";
import styles from "./SimpleNavigation.module.css";

/**
 * The one always-visible surface simplified mode has for a waiting card.
 *
 * Simplified mode does not mount the `DecisionPanel` — cards are answered
 * inline in the task's own dialogue instead (`SessionDetail`), which only
 * renders the ones belonging to the task you have open. So a card raised on any
 * other task used to have no surface at all: you heard the chime and there was
 * nothing on screen anywhere, and the only clue was the 「待决策」chip on that
 * task's row, which you had to go looking for. This pill says how many are
 * waiting no matter which page you are on, and clicking it opens the oldest
 * one's task (`requestOpenTask` hops to 任务 and selects the tab), where the
 * card is rendered.
 */
function PendingDecisionsPill() {
  const { t } = useTranslation();
  const decisions = useDecisionStore((s) => s.decisions);
  const requestOpenTask = useUIStore((s) => s.requestOpenTask);

  if (decisions.length === 0) return null;

  const oldest = oldestPendingDecision(decisions);
  const parked = decisions.some((d) => (d.request as { parked?: boolean }).parked === true);
  const target = oldest?.request as
    | { sessionId?: string; workspaceName?: string; aiTitle?: string | null }
    | undefined;
  // Name the task the click lands on, so pressing it is not a blind jump.
  const where = [target?.workspaceName, target?.aiTitle].filter(Boolean).join(" · ");

  return (
    <button
      type="button"
      className={parked ? styles.decisions_parked : styles.decisions}
      // Announce arrivals for a screen reader too: the chime is the only other
      // signal this pill exists to explain.
      aria-live="polite"
      title={
        where
          ? t("simple_nav.decisions_title_where", "点击打开：{{where}}", { where })
          : t("simple_nav.decisions_title", "点击打开等你回复的任务")
      }
      onClick={() => {
        if (target?.sessionId) requestOpenTask(target.sessionId);
      }}
    >
      {/* `n`, not `count` — `count` would put i18next into plural-resolution
          mode and need per-locale `_one`/`_other` keys for one short label. */}
      {parked
        ? t("simple_nav.decisions_parked", "{{n}} 张卡已超时", { n: decisions.length })
        : t("simple_nav.decisions_pending", "{{n}} 张卡等你回复", { n: decisions.length })}
    </button>
  );
}

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
      <PendingDecisionsPill />
      <button type="button" className={styles.settings} aria-label={t("settings.title")}
        title={t("settings.title")} onClick={() => setSettingsOpen(true)}>
        <Settings size={18} />
      </button>
    </header>
  );
}
