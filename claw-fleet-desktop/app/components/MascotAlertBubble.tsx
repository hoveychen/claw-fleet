import { useTranslation } from "react-i18next";
import {
  useDetailStore,
  useSessionsStore,
  useUIStore,
  useWaitingAlertsStore,
} from "../store";
import styles from "./MascotAlertBubble.module.css";

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
  if (visible.length === 0) return null;

  const latest = visible[0];
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

  return (
    <div
      className={styles.bubble}
      onClick={goToSession}
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
        ✕
      </button>
    </div>
  );
}
