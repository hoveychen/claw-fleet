/**
 * OverlayMascot — floating overlay window content.
 *
 * - Quip bubble OR alert cards above the frame (mutually exclusive)
 * - MascotEyes inside a rounded robot frame
 * - Status LED bar with labels
 * - Drag handle in bottom-right corner
 * - Double-click on frame opens main window
 * - Right-click hides overlay
 * - ResizeObserver for accurate window sizing
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { useSessionsStore, useWaitingAlertsStore } from "../store";
import type { WaitingAlert } from "../types";
import { getItem, setItem } from "../storage";
import { MascotEyesCore } from "./MascotEyesCore";
import styles from "./OverlayMascot.module.css";

function timeAgo(ms: number, t: (key: string, opts?: Record<string, unknown>) => string): string {
  const secs = Math.floor((Date.now() - ms) / 1000);
  if (secs < 60) return t("just_now");
  const mins = Math.floor(secs / 60);
  if (mins < 60) return t("m_ago", { n: mins });
  const hours = Math.floor(mins / 60);
  return t("h_ago", { n: hours });
}

function OverlayAlertCard({
  alert,
  onDismiss,
  onClick,
}: {
  alert: WaitingAlert;
  onDismiss: () => void;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const [leaving, setLeaving] = useState(false);

  const handleDismiss = (e: React.MouseEvent) => {
    e.stopPropagation();
    setLeaving(true);
    setTimeout(onDismiss, 280);
  };

  return (
    <div
      className={`${styles.alertCard} ${leaving ? styles.alertCard_leaving : ""}`}
      onClick={onClick}
    >
      <div className={styles.alertDot} />
      <div className={styles.alertContent}>
        <div className={styles.alertWorkspace}>{alert.workspaceName}</div>
        <div className={styles.alertSummary}>{alert.summary}</div>
        <div className={styles.alertTime}>{timeAgo(alert.detectedAtMs, t)}</div>
      </div>
      <button className={styles.alertClose} onClick={handleDismiss} aria-label="Dismiss">
        ✕
      </button>
    </div>
  );
}

export function OverlayMascot() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const { alerts, dismissedIds, dismiss } = useWaitingAlertsStore();
  const visibleAlerts = alerts.filter((a) => !dismissedIds.has(a.sessionId));
  const hasAlerts = visibleAlerts.length > 0;

  const [quipText, setQuipText] = useState<string | null>(null);
  const [muted, setMuted] = useState(() => getItem("overlay-muted") === "true");
  const [faceHidden, setFaceHidden] = useState(false);

  const toggleMute = useCallback(() => {
    setMuted((prev) => {
      const next = !prev;
      setItem("overlay-muted", next ? "true" : "false");
      return next;
    });
  }, []);

  const toggleFace = useCallback(() => {
    setFaceHidden((prev) => !prev);
  }, []);

  // Derive status counts — distinguish main vs sub-agent
  const busyStatuses = ["thinking", "executing", "streaming", "processing", "active", "delegating"];
  const mainSessions = sessions.filter((s) => !s.isSubagent);
  const subSessions = sessions.filter((s) => s.isSubagent);
  const mainBusyCount = mainSessions.filter((s) => busyStatuses.includes(s.status)).length;
  const subBusyCount = subSessions.filter((s) => busyStatuses.includes(s.status)).length;
  const waitingCount = mainSessions.filter((s) => s.status === "waitingInput").length;
  const totalSpeed = sessions.reduce((sum, s) => sum + s.tokenSpeed, 0);

  // Resize window to fit content via ResizeObserver
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const height = Math.ceil(el.scrollHeight) + 4; // small padding to prevent clipping
      getCurrentWindow().setSize(new LogicalSize(280, height)).catch(() => {});
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const handleDoubleClick = useCallback(() => {
    invoke("show_main_window").catch(() => {});
  }, []);

  const handleAlertClick = (alert: WaitingAlert) => {
    invoke("show_main_window").catch(() => {});
    invoke("open_session_from_overlay", { jsonlPath: alert.jsonlPath }).catch(() => {});
  };

  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    invoke("toggle_overlay", { visible: false }).catch(() => {});
    emit("overlay-disabled").catch(() => {});
  }, []);

  // Show at most 3 alerts, rest collapsed
  const shownAlerts = visibleAlerts.slice(0, 3);
  const moreCount = visibleAlerts.length - 3;

  // Show quip bubble only when there are no alerts
  const showQuip = !hasAlerts && !!quipText;

  return (
    <div className={styles.root} ref={rootRef}>
      {/* Alert cards above the frame — only when there are alerts */}
      {hasAlerts && (
        <div className={styles.alertStack}>
          {shownAlerts.map((alert) => (
            <OverlayAlertCard
              key={alert.sessionId}
              alert={alert}
              onDismiss={() => dismiss(alert.sessionId)}
              onClick={() => handleAlertClick(alert)}
            />
          ))}
          {moreCount > 0 && (
            <div className={styles.alertMore}>
              +{moreCount} {t("overlay.more_alerts")}
            </div>
          )}
        </div>
      )}

      {/* Quip bubble above the frame — only when no alerts and face visible */}
      {showQuip && !faceHidden && (
        <div className={styles.quipBubble}>
          <div className={styles.quipText}>{quipText}</div>
          <div className={styles.quipTail} />
        </div>
      )}

      {/* Robot frame */}
      <div
        className={styles.frame}
        onDoubleClick={handleDoubleClick}
        onContextMenu={handleContextMenu}
      >
        {/* Screen area with mascot eyes — hidden when faceHidden */}
        {!faceHidden && (
          <div className={styles.screen}>
            <MascotEyesCore onQuip={setQuipText} />
          </div>
        )}

        {/* Status LED bar with labels */}
        <div className={styles.statusBar} data-tauri-drag-region>
          {/* Control buttons — bottom left */}
          <div className={styles.controlButtons}>
            <button
              className={`${styles.controlBtn} ${muted ? styles.controlBtnActive : ""}`}
              onClick={toggleMute}
              title={muted ? t("overlay.unmute") : t("overlay.mute")}
              aria-label={muted ? t("overlay.unmute") : t("overlay.mute")}
            >
              {muted ? (
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <line x1="1" y1="1" x2="23" y2="23" />
                  <path d="M9 9v3a3 3 0 0 0 5.12 2.12M15 9.34V4a3 3 0 0 0-5.94-.6" />
                  <path d="M17 16.95A7 7 0 0 1 5 12v-2m14 0v2c0 .76-.12 1.5-.34 2.18" />
                  <line x1="12" y1="19" x2="12" y2="23" />
                  <line x1="8" y1="23" x2="16" y2="23" />
                </svg>
              ) : (
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
                  <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
                  <line x1="12" y1="19" x2="12" y2="23" />
                  <line x1="8" y1="23" x2="16" y2="23" />
                </svg>
              )}
            </button>
            <button
              className={`${styles.controlBtn} ${faceHidden ? styles.controlBtnActive : ""}`}
              onClick={toggleFace}
              title={faceHidden ? t("overlay.show_face") : t("overlay.hide_face")}
              aria-label={faceHidden ? t("overlay.show_face") : t("overlay.hide_face")}
            >
              {faceHidden ? (
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
                  <line x1="1" y1="1" x2="23" y2="23" />
                </svg>
              ) : (
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
                  <circle cx="12" cy="12" r="3" />
                </svg>
              )}
            </button>
          </div>

          {mainBusyCount > 0 && (
            <div className={styles.ledGroup}>
              <span className={`${styles.led} ${styles.ledActive}`} />
              <span className={styles.ledLabel}>
                {mainBusyCount} {t("overlay.led_busy")}
              </span>
            </div>
          )}
          {subBusyCount > 0 && (
            <div className={styles.ledGroup}>
              <span className={`${styles.led} ${styles.ledSub}`} />
              <span className={styles.ledLabel}>
                {subBusyCount} {t("overlay.led_sub")}
              </span>
            </div>
          )}
          {waitingCount > 0 && (
            <div className={styles.ledGroup}>
              <span className={`${styles.led} ${styles.ledWaiting}`} />
              <span className={styles.ledLabel}>
                {waitingCount} {t("overlay.led_waiting")}
              </span>
            </div>
          )}
          {totalSpeed > 0 && (
            <div className={styles.ledGroup}>
              <span className={styles.ledLabel}>
                {Math.round(totalSpeed)} {t("tok_s")}
              </span>
            </div>
          )}
          {sessions.length === 0 && (
            <span className={styles.ledLabel}>{t("overlay.no_agents")}</span>
          )}

          {/* Drag handle icon — bottom right (visual indicator only, whole bar is draggable) */}
          <div className={styles.dragHandle}>
            <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor" opacity="0.4">
              <circle cx="6" cy="2" r="1" />
              <circle cx="9" cy="2" r="1" />
              <circle cx="3" cy="5" r="1" />
              <circle cx="6" cy="5" r="1" />
              <circle cx="9" cy="5" r="1" />
              <circle cx="6" cy="8" r="1" />
              <circle cx="9" cy="8" r="1" />
            </svg>
          </div>
        </div>
      </div>
    </div>
  );
}
