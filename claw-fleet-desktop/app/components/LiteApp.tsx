import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  openSettingsWindow,
  useDecisionStore,
  useDetailStore,
  useSessionsStore,
  useUIStore,
  useWaitingAlertsStore,
} from "../store";
import type { SessionInfo } from "../types";
import { getItem, setItem } from "../storage";
import { CostSpeedChart } from "./CostSpeedChart";
import { DecisionPanel } from "./DecisionPanel";
import { LiteDecisionHistory } from "./LiteDecisionHistory";
import { LiteSessionCard } from "./LiteSessionCard";
import { MascotAlertBubble } from "./MascotAlertBubble";
import { MascotEyes } from "./MascotEyes";
import { useUsageRing } from "../hooks/useUsageRing";
import { MobileAccessPanel } from "./MobileAccessPanel";
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

export function LiteApp() {
  const { t } = useTranslation();
  const { sessions, setSessions, refresh } = useSessionsStore();
  const { open, session: openedSession } = useDetailStore();
  const {
    setLiteMode,
    showMobileAccess,
    setShowMobileAccess,
    liteDecisionHistorySessionId,
  } = useUIStore();
  const hasDecision = useDecisionStore((s) => s.decisions.length > 0);
  const hasAlerts = useWaitingAlertsStore(
    (s) => s.alerts.some((a) => !s.dismissedIds.has(a.sessionId)),
  );
  const [mobileActive, setMobileActive] = useState(false);
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

  // Poll mobile access status for the icon-bar indicator dot.
  useEffect(() => {
    const check = () => {
      invoke<{ running: boolean; tunnelUrl: string | null }>("get_mobile_access_status")
        .then((s) => setMobileActive(s.running && !!s.tunnelUrl))
        .catch(() => {});
    };
    check();
    const interval = setInterval(check, 5000);
    return () => clearInterval(interval);
  }, []);

  const active = sessions.filter((s) =>
    ACTIVE_STATUSES.includes(s.status as typeof ACTIVE_STATUSES[number]),
  );

  const openSession = (s: typeof active[number]) => {
    // Stay in lite mode — render SessionDetail inline instead of the drawer.
    open(s).catch(() => {});
  };

  return (
    <div className={styles.lite}>
      <div className={styles.drag_bar} data-tauri-drag-region>
        <span className={styles.drag_title} data-tauri-drag-region>
          {t("title")}
        </span>
        <button
          className={`${styles.icon_btn} ${mobileActive ? styles.icon_btn_active : ""}`}
          title={t("settings.mobile_access")}
          onClick={() => setShowMobileAccess(true)}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
            <rect x="4" y="1" width="8" height="14" rx="1.5" />
            <line x1="7" y1="12" x2="9" y2="12" />
          </svg>
          {mobileActive && <span className={styles.icon_btn_dot} />}
        </button>
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

      {showMobileAccess && <MobileAccessPanel onClose={() => setShowMobileAccess(false)} />}

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
      ) : hasDecision ? (
        <DecisionPanel compact />
      ) : openedSession ? (
        <div className={styles.detail_area}>
          <SessionDetail lite />
        </div>
      ) : (
        <div className={styles.body}>
          <div className={styles.monitor}>
            <TokenSpeedChart compact />
            <CostSpeedChart compact />
          </div>

          <div className={styles.list}>
            {active.length > 0 ? (
              active.map((s, i) => (
                <LiteSessionCard
                  key={s.jsonlPath}
                  session={s}
                  nextIsSubagent={active[i + 1]?.isSubagent === true}
                  onClick={() => openSession(s)}
                />
              ))
            ) : (
              <p className={styles.empty}>{t("no_sessions")}</p>
            )}
          </div>

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
        </div>
      )}
    </div>
  );
}
