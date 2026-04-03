/**
 * OverlayMascot — floating overlay window content.
 *
 * - Quip bubble OR alert cards above the frame (mutually exclusive)
 * - MascotEyes inside a rounded robot frame
 * - Status LED bar with labels
 * - Drag handle in bottom-right corner
 * - Double-click on frame opens main window
 * - Right-click opens context menu (open main, active agent, mute, hide, recall)
 * - ResizeObserver for accurate window sizing
 */

import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { useSessionsStore, useWaitingAlertsStore } from "../store";
import type { WaitingAlert } from "../types";
import { getItem, setItem } from "../storage";
import { MascotEyesCore } from "./MascotEyesCore";
import { RobotFrame } from "./RobotFrame";
import styles from "./OverlayMascot.module.css";

/** Error boundary so MascotEyes crashes don't kill the entire overlay (including alert cards). */
class MascotErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { hasError: boolean }
> {
  state = { hasError: false };
  static getDerivedStateFromError() { return { hasError: true }; }
  componentDidCatch(err: unknown) { console.error("[overlay] MascotEyes crashed:", err); }
  render() {
    if (this.state.hasError) return null;
    return this.props.children;
  }
}

function timeAgo(ms: number, t: (key: string, opts?: Record<string, unknown>) => string): string {
  const secs = Math.floor((Date.now() - ms) / 1000);
  if (secs < 60) return t("just_now");
  const mins = Math.floor(secs / 60);
  if (mins < 60) return t("m_ago", { n: mins });
  const hours = Math.floor(mins / 60);
  return t("h_ago", { n: hours });
}

const ALERT_DISMISS_MS = 15_000; // auto-dismiss after 15s

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
  const [progress, setProgress] = useState(1); // 1 = full, 0 = expired
  const [paused, setPaused] = useState(false);
  const progressRef = useRef(1);
  const pausedRef = useRef(false);
  const onDismissRef = useRef(onDismiss);
  useEffect(() => { onDismissRef.current = onDismiss; }, [onDismiss]);

  // Keep refs in sync
  useEffect(() => { pausedRef.current = paused; }, [paused]);

  // Countdown timer using requestAnimationFrame for smooth ring animation
  useEffect(() => {
    let raf: number;
    let lastTime = performance.now();
    progressRef.current = 1;
    setProgress(1);

    const tick = (now: number) => {
      const dt = now - lastTime;
      lastTime = now;
      if (!pausedRef.current) {
        progressRef.current -= dt / ALERT_DISMISS_MS;
        if (progressRef.current <= 0) {
          progressRef.current = 0;
          setProgress(0);
          setLeaving(true);
          setTimeout(() => onDismissRef.current(), 280);
          return;
        }
        setProgress(progressRef.current);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, []);

  const handleDismiss = (e: React.MouseEvent) => {
    e.stopPropagation();
    setLeaving(true);
    setTimeout(onDismiss, 280);
  };

  // SVG stamina ring params
  const ringSize = 18;
  const ringR = 7;
  const ringCirc = 2 * Math.PI * ringR;
  const ringOffset = ringCirc * (1 - progress);

  return (
    <div
      className={`${styles.alertCard} ${leaving ? styles.alertCard_leaving : ""}`}
      onClick={onClick}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
    >
      <div className={styles.alertDot} />
      <div className={styles.alertContent}>
        <div className={styles.alertWorkspace}>{alert.workspaceName}</div>
        <div className={styles.alertSummary}>{alert.summary}</div>
        <div className={styles.alertTime}>{timeAgo(alert.detectedAtMs, t)}</div>
      </div>
      <button className={styles.alertClose} onClick={handleDismiss} aria-label="Dismiss">
        <svg className={styles.countdownRing} width={ringSize} height={ringSize} viewBox={`0 0 ${ringSize} ${ringSize}`}>
          {/* Background track */}
          <circle
            cx={ringSize / 2} cy={ringSize / 2} r={ringR}
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            opacity="0.15"
          />
          {/* Depleting arc */}
          <circle
            cx={ringSize / 2} cy={ringSize / 2} r={ringR}
            fill="none"
            stroke={progress < 0.25 ? "#ef4444" : progress < 0.5 ? "#fbbf24" : "#4ade80"}
            strokeWidth="1.5"
            strokeDasharray={ringCirc}
            strokeDashoffset={ringOffset}
            strokeLinecap="round"
            transform={`rotate(-90 ${ringSize / 2} ${ringSize / 2})`}
            style={{ transition: paused ? "stroke 0.3s" : "none" }}
          />
        </svg>
        <span className={styles.countdownX}>✕</span>
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
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number } | null>(null);
  const ctxRef = useRef<HTMLDivElement>(null);

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

  // Close context menu on outside click or escape
  useEffect(() => {
    if (!ctxMenu) return;
    const handleClick = (e: globalThis.MouseEvent) => {
      if (ctxRef.current && !ctxRef.current.contains(e.target as Node)) {
        setCtxMenu(null);
      }
    };
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setCtxMenu(null);
    };
    window.addEventListener("mousedown", handleClick);
    window.addEventListener("keydown", handleKey);
    return () => {
      window.removeEventListener("mousedown", handleClick);
      window.removeEventListener("keydown", handleKey);
    };
  }, [ctxMenu]);

  // Derive status counts — distinguish main vs sub-agent
  const busyStatuses = ["thinking", "executing", "streaming", "processing", "active", "delegating"];
  const mainSessions = sessions.filter((s) => !s.isSubagent);
  const subSessions = sessions.filter((s) => s.isSubagent);
  const mainBusyCount = mainSessions.filter((s) => busyStatuses.includes(s.status)).length;
  const subBusyCount = subSessions.filter((s) => busyStatuses.includes(s.status)).length;
  const waitingCount = mainSessions.filter((s) => s.status === "waitingInput").length;
  const totalSpeed = sessions.reduce((sum, s) => sum + s.tokenSpeed, 0);

  // Resize window to fit content via ResizeObserver, anchoring the bottom edge
  const rootRef = useRef<HTMLDivElement>(null);
  const prevHeightRef = useRef<number>(0);
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const win = getCurrentWindow();
    const ro = new ResizeObserver(async () => {
      const newHeight = Math.ceil(el.scrollHeight) + 4; // small padding to prevent clipping
      const oldHeight = prevHeightRef.current;
      prevHeightRef.current = newHeight;
      try {
        if (oldHeight > 0 && oldHeight !== newHeight) {
          // Shift window position so the bottom edge stays in place
          const pos = await win.outerPosition();
          const scaleFactor = await win.scaleFactor();
          const deltaLogical = newHeight - oldHeight;
          await win.setPosition(new LogicalPosition(pos.x / scaleFactor, pos.y / scaleFactor - deltaLogical));
        }
        await win.setSize(new LogicalSize(280, newHeight));
      } catch { /* ignore */ }
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

  // Find the most recently active main agent
  const busyMainAgent = mainSessions
    .filter((s) => busyStatuses.includes(s.status))
    .sort((a, b) => b.lastActivityMs - a.lastActivityMs)[0] ?? null;

  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    // Clamp so menu stays within the overlay window
    const menuW = 190;
    const menuH = 200;
    const x = Math.min(e.clientX, window.innerWidth - menuW);
    const y = Math.max(0, Math.min(e.clientY, window.innerHeight - menuH));
    setCtxMenu({ x, y });
  }, []);

  const handleCtxOpenMain = useCallback(() => {
    setCtxMenu(null);
    invoke("show_main_window").catch(() => {});
  }, []);

  const handleCtxOpenAgent = useCallback(() => {
    if (!busyMainAgent) return;
    setCtxMenu(null);
    invoke("show_main_window").catch(() => {});
    invoke("open_session_from_overlay", { jsonlPath: busyMainAgent.jsonlPath }).catch(() => {});
  }, [busyMainAgent]);

  const handleCtxMute = useCallback(() => {
    setCtxMenu(null);
    toggleMute();
  }, [toggleMute]);

  const handleCtxHide = useCallback(() => {
    setCtxMenu(null);
    invoke("toggle_overlay", { visible: false }).catch(() => {});
  }, []);

  const handleCtxRecall = useCallback(() => {
    setCtxMenu(null);
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
      <RobotFrame
        className={styles.overlayFrame}
        tauriDrag
        onDoubleClick={handleDoubleClick}
        onContextMenu={handleContextMenu}
        footer={
          <div className={styles.statusBar}>
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
              <div className={styles.ledGroup} title={t("overlay.led_busy")}>
                {/* Gear icon — spinning when active */}
                <svg className={`${styles.ledIcon} ${styles.ledIconBusy}`} width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <circle cx="12" cy="12" r="3" />
                  <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
                </svg>
                <span className={styles.ledLabel}>{mainBusyCount}</span>
              </div>
            )}
            {subBusyCount > 0 && (
              <div className={styles.ledGroup} title={t("overlay.led_sub")}>
                {/* Git-branch icon — forked sub-tasks */}
                <svg className={`${styles.ledIcon} ${styles.ledIconSub}`} width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <line x1="6" y1="3" x2="6" y2="15" />
                  <circle cx="18" cy="6" r="3" />
                  <circle cx="6" cy="18" r="3" />
                  <path d="M18 9a9 9 0 0 1-9 9" />
                </svg>
                <span className={styles.ledLabel}>{subBusyCount}</span>
              </div>
            )}
            {waitingCount > 0 && (
              <div className={styles.ledGroup} title={t("overlay.led_waiting")}>
                {/* Pause icon — waiting for input */}
                <svg className={`${styles.ledIcon} ${styles.ledIconWaiting}`} width="10" height="10" viewBox="0 0 24 24" fill="currentColor">
                  <rect x="5" y="3" width="5" height="18" rx="1" />
                  <rect x="14" y="3" width="5" height="18" rx="1" />
                </svg>
                <span className={styles.ledLabel}>{waitingCount}</span>
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

            {/* Collapse button — hides the overlay */}
            <button
              className={styles.collapseBtn}
              onClick={handleCtxRecall}
              title={t("overlay.recall")}
              aria-label={t("overlay.recall")}
            >
              <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                <path d="M12 9v4a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h4" />
                <path d="M9 7L2 14" />
                <path d="M2 10v4h4" />
              </svg>
            </button>
          </div>
        }
      >
        {!faceHidden && (
          <MascotErrorBoundary>
            <MascotEyesCore onQuip={setQuipText} />
          </MascotErrorBoundary>
        )}
      </RobotFrame>

      {/* Context menu */}
      {ctxMenu && (
        <div
          ref={ctxRef}
          className={styles.ctxMenu}
          style={{ left: ctxMenu.x, top: ctxMenu.y }}
        >
          {/* Open main window */}
          <button className={styles.ctxItem} onClick={handleCtxOpenMain}>
            <svg className={styles.ctxItemIcon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
              <line x1="3" y1="9" x2="21" y2="9" />
            </svg>
            {t("overlay.ctx_open_main")}
          </button>

          {/* Active main agent */}
          <button
            className={`${styles.ctxItem} ${!busyMainAgent ? styles.ctxItemDisabled : ""}`}
            onClick={handleCtxOpenAgent}
            disabled={!busyMainAgent}
          >
            <svg className={styles.ctxItemIcon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
            <span className={styles.ctxActiveLabel}>
              {busyMainAgent
                ? t("overlay.ctx_active_agent", { name: busyMainAgent.workspaceName })
                : t("overlay.ctx_no_active")}
            </span>
          </button>

          <div className={styles.ctxSeparator} />

          {/* Mute toggle */}
          <button className={styles.ctxItem} onClick={handleCtxMute}>
            <svg className={styles.ctxItemIcon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              {muted ? (
                <>
                  <line x1="1" y1="1" x2="23" y2="23" />
                  <path d="M9 9v3a3 3 0 0 0 5.12 2.12M15 9.34V4a3 3 0 0 0-5.94-.6" />
                  <path d="M17 16.95A7 7 0 0 1 5 12v-2m14 0v2c0 .76-.12 1.5-.34 2.18" />
                  <line x1="12" y1="19" x2="12" y2="23" />
                  <line x1="8" y1="23" x2="16" y2="23" />
                </>
              ) : (
                <>
                  <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
                  <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
                  <line x1="12" y1="19" x2="12" y2="23" />
                  <line x1="8" y1="23" x2="16" y2="23" />
                </>
              )}
            </svg>
            {muted ? t("overlay.unmute") : t("overlay.mute")}
          </button>

          {/* Hide overlay */}
          <button className={styles.ctxItem} onClick={handleCtxHide}>
            <svg className={styles.ctxItemIcon} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
              <line x1="1" y1="1" x2="23" y2="23" />
            </svg>
            {t("overlay.ctx_hide")}
          </button>

          {/* Recall to sidebar */}
          <button className={styles.ctxItem} onClick={handleCtxRecall}>
            <svg className={styles.ctxItemIcon} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 9v4a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h4" />
              <path d="M9 7L2 14" />
              <path d="M2 10v4h4" />
            </svg>
            {t("overlay.ctx_recall")}
          </button>
        </div>
      )}
    </div>
  );
}
