import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import type { WaitingAlert } from "../types";
import { useWaitingAlertsStore } from "../store";
import styles from "./WaitingAlerts.module.css";

function timeAgo(ms: number, t: (key: string, opts?: Record<string, unknown>) => string): string {
  const secs = Math.floor((Date.now() - ms) / 1000);
  if (secs < 60) return t("just_now");
  const mins = Math.floor(secs / 60);
  return t("m_ago", { n: mins });
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

  return (
    <div
      ref={cardRef}
      className={`${styles.card} ${leaving ? styles.card_leaving : ""}`}
      onClick={handleDismiss}
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

/**
 * Floating notification stack — rendered as an overlay in the bottom-right
 * of the main content area.
 */
export function WaitingAlerts() {
  const { alerts, setAlerts, refresh, dismiss, dismissedIds } = useWaitingAlertsStore();

  useEffect(() => {
    refresh();
    const unlistenPromise = listen<WaitingAlert[]>("waiting-alerts-updated", (e) => {
      setAlerts(e.payload);
    });
    return () => {
      unlistenPromise.then((u) => u());
    };
  }, []);

  const visible = alerts.filter((a) => !dismissedIds.has(a.sessionId));

  if (visible.length === 0) return null;

  return (
    <div className={styles.overlay}>
      {visible.map((alert) => (
        <AlertCard
          key={alert.sessionId}
          alert={alert}
          onDismiss={() => dismiss(alert.sessionId)}
        />
      ))}
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
