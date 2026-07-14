import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  openSettingsWindow,
  useDetailStore,
  useSessionsStore,
  useUIStore,
  useWaitingAlertsStore,
} from "../store";
import type { SessionInfo } from "../types";
import { isWorkflowAgent } from "../workflowAgent";
import { getItem, setItem } from "../storage";
import { CostSpeedChart } from "./CostSpeedChart";
import { LiteDecisionHistory } from "./LiteDecisionHistory";
import { LiteSessionCard } from "./LiteSessionCard";
import { MascotAlertBubble } from "./MascotAlertBubble";
import { MascotEyes } from "./MascotEyes";
import { useUsageRing } from "../hooks/useUsageRing";
import { SessionDetail } from "./SessionDetail";
import { TokenSpeedChart } from "./TokenSpeedChart";
import { UsagePanel } from "./UsagePanel";
import styles from "./LiteApp.module.css";

const ACTIVE_STATUSES = [
  "thinking",
  "executing",
  "streaming",
  "processing",
  "waitingInput",
  "active",
  "delegating",
] as const;

/**
 * Lite mode = a phone-shaped portrait strip that mirrors the desktop's task
 * page in a mobile form factor:
 *
 *   • List page  — a top overview strip (charts + mascot/usage ring) followed
 *     by the full session list (active + recent). The whole page scrolls, so
 *     scrolling down past the overview reveals the complete task list.
 *   • Detail page — tapping a task pushes SessionDetail over the full window;
 *     SessionDetail's own close button (rendered because `lite && !inline`)
 *     acts as the back affordance.
 *
 * Decision cards are NOT rendered here — in lite mode they always pop out as
 * the standalone `decision-float` window (see App.tsx's float bridge).
 */
export function LiteApp() {
  const { t } = useTranslation();
  const { sessions, setSessions, refresh } = useSessionsStore();
  const { open, session: openedSession } = useDetailStore();
  const {
    setLiteMode,
    liteDecisionHistorySessionId,
    mascotVisible,
  } = useUIStore();
  const hasAlerts = useWaitingAlertsStore(
    (s) => s.alerts.some((a) => !s.dismissedIds.has(a.sessionId)),
  );
  const [ttsMuted, setTtsMuted] = useState(() => getItem("tts-muted") === "true");
  const [showUsage, setShowUsage] = useState(false);
  const usageRing = useUsageRing();

  const toggleTtsMuted = () => {
    const next = !ttsMuted;
    setTtsMuted(next);
    setItem("tts-muted", next ? "true" : "false");
  };

  // Keep session list flowing in lite mode (normal SessionList is unmounted).
  useEffect(() => {
    const unlistenPromise = listen<SessionInfo[]>("sessions-updated", (e) => {
      setSessions(e.payload);
    });
    refresh().catch(() => {});
    return () => {
      unlistenPromise.then((u) => u());
    };
  }, [setSessions, refresh]);

  // Task list mirrors the desktop task page: active sessions first, then the
  // most recent idle ones. Workflow agents are hidden (opened via DAG node).
  const visible = sessions.filter((s) => !isWorkflowAgent(s));
  const active = visible.filter((s) =>
    ACTIVE_STATUSES.includes(s.status as typeof ACTIVE_STATUSES[number]),
  );
  const idle = visible
    .filter((s) => s.status === "idle")
    .sort((a, b) => b.lastActivityMs - a.lastActivityMs);

  const openSession = (s: SessionInfo) => {
    // Push the detail over the full window; SessionDetail's close button
    // brings us back to the list.
    open(s).catch(() => {});
  };

  const renderGroup = (list: SessionInfo[]) =>
    list.map((s, i) => (
      <LiteSessionCard
        key={s.jsonlPath}
        session={s}
        nextIsSubagent={list[i + 1]?.isSubagent === true}
        onClick={() => openSession(s)}
      />
    ));

  return (
    <div className={styles.lite}>
      <div className={styles.drag_bar} data-tauri-drag-region>
        <span className={styles.drag_title} data-tauri-drag-region>
          {t("title")}
        </span>
        <button
          className={styles.icon_btn}
          title={t(ttsMuted ? "lite.unmute" : "lite.mute")}
          onClick={toggleTtsMuted}
        >
          {ttsMuted ? (
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
              <path d="M8 3L5 6H2v4h3l3 3z" />
              <line x1="11" y1="6" x2="15" y2="10" />
              <line x1="15" y1="6" x2="11" y2="10" />
            </svg>
          ) : (
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
              <path d="M8 3L5 6H2v4h3l3 3z" />
              <path d="M11.5 5.5a3.5 3.5 0 0 1 0 5" />
              <path d="M13.5 3.5a6 6 0 0 1 0 9" />
            </svg>
          )}
        </button>
        <button
          className={styles.icon_btn}
          title={t("settings.title")}
          onClick={openSettingsWindow}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="8" cy="8" r="1.5" />
            <path d="M6.7 1.2l-.4 1.6a5 5 0 0 0-1.5.9L3.3 3.2 1.9 5.6l1.2 1.1a5 5 0 0 0 0 1.7l-1.2 1.1 1.4 2.4 1.5-.5a5 5 0 0 0 1.5.9l.4 1.6h2.6l.4-1.6a5 5 0 0 0 1.5-.9l1.5.5 1.4-2.4-1.2-1.1a5 5 0 0 0 0-1.7l1.2-1.1-1.4-2.4-1.5.5a5 5 0 0 0-1.5-.9L9.3 1.2z" />
          </svg>
        </button>
        <button
          className={styles.icon_btn}
          title={t("lite.exit")}
          onClick={() => setLiteMode(false)}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
            <path d="M4 6 L4 3 L13 3 L13 13 L4 13 L4 10" />
            <path d="M9 8 L2 8 M5 5 L2 8 L5 11" />
          </svg>
        </button>
      </div>

      {showUsage && (
        <div className={styles.usage_overlay} onClick={() => setShowUsage(false)}>
          <div className={styles.usage_panel} onClick={(e) => e.stopPropagation()}>
            <button
              className={styles.usage_close}
              onClick={() => setShowUsage(false)}
              aria-label="Close"
            >×</button>
            <UsagePanel />
          </div>
        </div>
      )}

      {liteDecisionHistorySessionId ? (
        <div className={styles.detail_area}>
          <LiteDecisionHistory sessionId={liteDecisionHistorySessionId} />
        </div>
      ) : openedSession ? (
        <div className={styles.detail_area}>
          <SessionDetail lite />
        </div>
      ) : (
        <div className={styles.body}>
          {/* Overview strip — kept from the old dashboard, now anchored at the
              top of the scrolling list page. */}
          <div className={styles.overview}>
            <div className={styles.monitor}>
              <TokenSpeedChart compact />
              <CostSpeedChart compact />
            </div>

            {mascotVisible && (
              <div className={styles.mascot_slot}>
                <MascotAlertBubble />
                <MascotEyes
                  suppressQuip={hasAlerts}
                  dashboardMode
                  usageRing={usageRing ? {
                    percent: usageRing.overall,
                    topSource: usageRing.topSource,
                    sources: usageRing.sources,
                    onClick: () => setShowUsage(true),
                  } : null}
                />
              </div>
            )}
          </div>

          {/* Task list — the sidebar, mobile-style. */}
          <div className={styles.list}>
            {active.length === 0 && idle.length === 0 ? (
              <p className={styles.empty}>{t("no_sessions")}</p>
            ) : (
              <>
                {active.length > 0 && (
                  <section className={styles.group}>
                    <div className={styles.group_label}>{t("active")}</div>
                    {renderGroup(active)}
                  </section>
                )}
                {idle.length > 0 && (
                  <section className={styles.group}>
                    <div className={styles.group_label}>{t("recent")}</div>
                    {renderGroup(idle)}
                  </section>
                )}
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
