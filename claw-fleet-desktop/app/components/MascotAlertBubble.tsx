import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  useDetailStore,
  useSessionsStore,
  useUIStore,
  useWaitingAlertsStore,
} from "../store";
import styles from "./MascotAlertBubble.module.css";

const ALERT_DISMISS_MS = 15_000;

function timeAgo(ms: number, t: (key: string, opts?: Record<string, unknown>) => string): string {
  const secs = Math.floor((Date.now() - ms) / 1000);
  if (secs < 60) return t("just_now");
  const mins = Math.floor(secs / 60);
  return t("m_ago", { n: mins });
}

export function MascotAlertBubble() {
  const { t } = useTranslation();
  const alerts = useWaitingAlertsStore((s) => s.alerts);
  const dismissedIds = useWaitingAlertsStore((s) => s.dismissedIds);
  const dismiss = useWaitingAlertsStore((s) => s.dismiss);

  const visible = alerts.filter((a) => !dismissedIds.has(a.sessionId));
  const latest = visible[0];

  const [progress, setProgress] = useState(1);
  const [paused, setPaused] = useState(false);
  const progressRef = useRef(1);
  const pausedRef = useRef(false);

  useEffect(() => { pausedRef.current = paused; }, [paused]);

  // Countdown — resets when the visible alert changes.
  useEffect(() => {
    if (!latest) return;
    let raf = 0;
    let last = performance.now();
    progressRef.current = 1;
    setProgress(1);
    const tick = (now: number) => {
      const dt = now - last;
      last = now;
      if (!pausedRef.current) {
        progressRef.current -= dt / ALERT_DISMISS_MS;
        if (progressRef.current <= 0) {
          progressRef.current = 0;
          setProgress(0);
          dismiss(latest.sessionId);
          return;
        }
        setProgress(progressRef.current);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [latest?.sessionId, dismiss]);

  if (!latest) return null;
  const extra = visible.length - 1;

  const goToSession = () => {
    const session = useSessionsStore.getState().sessions.find(
      (s) => s.jsonlPath === latest.jsonlPath,
    );
    if (session) {
      useUIStore.getState().setViewMode("list");
      useDetailStore.getState().open(session);
    }
    dismiss(latest.sessionId);
  };

  const ringSize = 18;
  const ringR = 7;
  const ringCirc = 2 * Math.PI * ringR;
  const ringOffset = ringCirc * (1 - progress);

  return (
    <div
      className={styles.bubble}
      onClick={goToSession}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      title={t("waiting_alerts.dismiss_tip")}
      key={latest.sessionId}
    >
      <span className={styles.dot} />
      <div className={styles.content}>
        <div className={styles.workspace}>{latest.workspaceName}</div>
        <div className={styles.summary}>{latest.summary}</div>
        <div className={styles.time}>{timeAgo(latest.detectedAtMs, t)}</div>
      </div>
      {extra > 0 && <span className={styles.badge}>+{extra}</span>}
      <button
        className={styles.close}
        onClick={(e) => {
          e.stopPropagation();
          dismiss(latest.sessionId);
        }}
        aria-label="Dismiss"
      >
        <svg
          className={styles.countdownRing}
          width={ringSize}
          height={ringSize}
          viewBox={`0 0 ${ringSize} ${ringSize}`}
        >
          <circle
            cx={ringSize / 2}
            cy={ringSize / 2}
            r={ringR}
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            opacity="0.2"
          />
          <circle
            cx={ringSize / 2}
            cy={ringSize / 2}
            r={ringR}
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
        <span className={styles.closeX}>✕</span>
      </button>
    </div>
  );
}
