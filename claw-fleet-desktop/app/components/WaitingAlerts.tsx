import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import type { WaitingAlert } from "../types";
import { useDecisionStore, useDetailStore, useSessionsStore, useUIStore, useWaitingAlertsStore } from "../store";
import { getItem } from "../storage";
import { playAlertSound, type TtsMode } from "../audio";
import styles from "./WaitingAlerts.module.css";

function timeAgo(ms: number, t: (key: string, opts?: Record<string, unknown>) => string): string {
  const secs = Math.floor((Date.now() - ms) / 1000);
  if (secs < 60) return t("just_now");
  const mins = Math.floor(secs / 60);
  return t("m_ago", { n: mins });
}

/** Navigate to a session by jsonlPath: switch to list view and open the session detail */
function navigateToSession(jsonlPath: string) {
  const session = useSessionsStore.getState().sessions.find((s) => s.jsonlPath === jsonlPath);
  if (session) {
    useUIStore.getState().setViewMode("list");
    useDetailStore.getState().open(session);
  }
}

/** A single notification card with swipe-to-dismiss */
function AlertCard({
  alert,
  onDismiss,
}: {
  alert: WaitingAlert;
  onDismiss: () => void;
}) {
  const { t } = useTranslation();
  const [leaving, setLeaving] = useState(false);
  const cardRef = useRef<HTMLDivElement>(null);

  const handleDismiss = () => {
    setLeaving(true);
    // Wait for exit animation to finish before removing from store
    setTimeout(onDismiss, 280);
  };

  const handleNavigate = () => {
    handleDismiss();
    navigateToSession(alert.jsonlPath);
  };

  return (
    <div
      ref={cardRef}
      className={`${styles.card} ${leaving ? styles.card_leaving : ""}`}
      onClick={handleNavigate}
      title={t("waiting_alerts.dismiss_tip")}
    >
      <div className={styles.card_dot} />
      <div className={styles.card_content}>
        <div className={styles.card_workspace}>{alert.workspaceName}</div>
        <div className={styles.card_summary}>{alert.summary}</div>
        <div className={styles.card_time}>{timeAgo(alert.detectedAtMs, t)}</div>
      </div>
      <button
        className={styles.card_close}
        onClick={(e) => {
          e.stopPropagation();
          handleDismiss();
        }}
        aria-label="Dismiss"
      >
        ✕
      </button>
    </div>
  );
}

export function WaitingAlerts() {
  const { alerts, setAlerts, refresh, dismiss, dismissedIds } = useWaitingAlertsStore();
  const liteMode = useUIStore((s) => s.liteMode);
  const hasDecision = useDecisionStore((s) => s.decisions.length > 0);
  const openedSession = useDetailStore((s) => s.session);
  const spokenIds = useRef(new Set<string>());

  useEffect(() => {
    refresh();
    const unlistenPromise = listen<WaitingAlert[]>("waiting-alerts-updated", (e) => {
      setAlerts(e.payload);

      // Chime / TTS for new alerts.
      //
      // Claude Code sessions MAY route their wait-for-input through the
      // AskUserQuestion → DecisionPanel bridge, which owns the audio cue
      // there.  When that bridge fires we must not double-announce. But
      // plain waitingInput (no AskUserQuestion) for claude-code still
      // needs a sound — otherwise Boss gets silently-pending sessions.
      // Strategy: defer the claude-code chime slightly; if a matching
      // decision arrives within the delay, cancel (DecisionPanel will
      // cover it); otherwise play.
      const ttsMode = (getItem("tts-mode") as TtsMode) || "off";
      if (ttsMode === "off") return;
      for (const alert of e.payload) {
        if (spokenIds.current.has(alert.sessionId)) continue;
        spokenIds.current.add(alert.sessionId);
        if (alert.source === "claude-code") {
          const { sessionId, summary } = alert;
          setTimeout(() => {
            const decisions = useDecisionStore.getState().decisions;
            const handledByPanel = decisions.some(
              (d) => d.request?.sessionId === sessionId,
            );
            if (!handledByPanel) playAlertSound(summary);
          }, 800);
        } else {
          playAlertSound(alert.summary);
        }
      }
    });

    return () => {
      unlistenPromise.then((u) => u());
    };
  }, []);

  const visible = alerts.filter((a) => !dismissedIds.has(a.sessionId));

  if (visible.length === 0) return null;
  // Lite mode body view delegates to MascotAlertBubble (speech bubble on the
  // mascot). Lite sub-views without the mascot (DecisionPanel, SessionDetail)
  // fall back to the bottom toast so alerts stay visible there too.
  const liteBodyView = liteMode && !hasDecision && !openedSession;
  if (liteBodyView) return null;

  const MAX_STACK = 5;
  const shown = visible.slice(0, MAX_STACK);
  const overflowCount = visible.length - MAX_STACK;

  return (
    <div className={`${styles.overlay} ${liteMode ? styles.overlay_lite : ""}`}>
      <div className={styles.stack}>
        {shown.map((alert, i) => (
          <div
            key={alert.sessionId}
            className={styles.stack_layer}
            style={{
              zIndex: MAX_STACK - i,
              transform: `translateY(${-i * 6}px) scale(${1 - i * 0.03})`,
              opacity: i === 0 ? 1 : Math.max(0.4, 1 - i * 0.15),
              pointerEvents: i === 0 ? "auto" : "none",
            }}
          >
            <AlertCard
              alert={alert}
              onDismiss={() => dismiss(alert.sessionId)}
            />
          </div>
        ))}
        {overflowCount > 0 && (
          <div className={styles.overflow_badge}>+{overflowCount}</div>
        )}
      </div>
    </div>
  );
}

/**
 * Red dot badge indicator — shows when there are undismissed alerts.
 * Use this in the sidebar or tray to draw attention.
 */
export function AlertBadge({ className }: { className?: string }) {
  const alerts = useWaitingAlertsStore((s) => s.alerts);
  const dismissedIds = useWaitingAlertsStore((s) => s.dismissedIds);
  const count = alerts.filter((a) => !dismissedIds.has(a.sessionId)).length;

  if (count === 0) return null;

  return (
    <span className={`${styles.badge} ${className ?? ""}`}>
      {count}
    </span>
  );
}
