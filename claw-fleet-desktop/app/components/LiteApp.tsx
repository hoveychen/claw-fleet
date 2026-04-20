import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  useDecisionStore,
  useDetailStore,
  useSessionsStore,
  useUIStore,
} from "../store";
import type { SessionInfo } from "../types";
import { CostSpeedChart } from "./CostSpeedChart";
import { DecisionPanel } from "./DecisionPanel";
import { LiteSessionCard } from "./LiteSessionCard";
import { MascotAlertBubble } from "./MascotAlertBubble";
import { MascotEyes } from "./MascotEyes";
import { SessionDetail } from "./SessionDetail";
import { TokenSpeedChart } from "./TokenSpeedChart";
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
  const { setLiteMode } = useUIStore();
  const hasDecision = useDecisionStore((s) => s.decisions.length > 0);

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
          className={styles.exit_btn}
          title={t("lite.exit")}
          onClick={() => setLiteMode(false)}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
            <path d="M4 6 L4 3 L13 3 L13 13 L4 13 L4 10" />
            <path d="M9 8 L2 8 M5 5 L2 8 L5 11" />
          </svg>
        </button>
      </div>

      {hasDecision ? (
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
              active.map((s) => (
                <LiteSessionCard
                  key={s.jsonlPath}
                  session={s}
                  onClick={() => openSession(s)}
                />
              ))
            ) : (
              <p className={styles.empty}>{t("no_sessions")}</p>
            )}
          </div>

          <div className={styles.mascot_slot}>
            <MascotAlertBubble />
            <MascotEyes />
          </div>
        </div>
      )}
    </div>
  );
}
