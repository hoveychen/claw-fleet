import { useTranslation } from "react-i18next";
import type { SessionInfo, SessionStatus } from "../types";
import styles from "./SessionCard.module.css";

// ── Status badge ─────────────────────────────────────────────────────────────

function StatusBadge({ status }: { status: SessionStatus }) {
  const { t } = useTranslation();
  return (
    <span className={`${styles.badge} ${styles[`badge_${status}`]}`}>
      {status === "streaming" || status === "processing" ? (
        <span className={styles.dot_anim} />
      ) : null}
      {t(`status.${status}`)}
    </span>
  );
}

// ── Token speed ───────────────────────────────────────────────────────────────

function TokenSpeed({ speed }: { speed: number }) {
  const { t } = useTranslation();
  if (speed < 0.5) return null;
  return (
    <span className={styles.speed}>
      {speed.toFixed(1)} <span className={styles.speed_unit}>{t("tok_s")}</span>
    </span>
  );
}

// ── Time ago ──────────────────────────────────────────────────────────────────

function TimeAgo({ ms }: { ms: number }) {
  const { t } = useTranslation();
  const diff = Date.now() - ms;
  let label: string;
  if (diff < 60_000) label = t("just_now");
  else if (diff < 3_600_000) label = t("m_ago", { n: Math.floor(diff / 60_000) });
  else if (diff < 86_400_000) label = t("h_ago", { n: Math.floor(diff / 3_600_000) });
  else label = t("d_ago", { n: Math.floor(diff / 86_400_000) });
  return <span className={styles.time}>{label}</span>;
}

// ── SessionCard ───────────────────────────────────────────────────────────────

interface Props {
  session: SessionInfo;
  isSelected: boolean;
  onClick: () => void;
}

export function SessionCard({ session, isSelected, onClick }: Props) {
  const { t } = useTranslation();
  const isActive = ["streaming", "processing", "waitingInput", "delegating"].includes(
    session.status
  );

  return (
    <div
      className={`${styles.card} ${isSelected ? styles.selected : ""} ${isActive ? styles.active : ""}`}
      onClick={onClick}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => e.key === "Enter" && onClick()}
    >
      {/* Header row */}
      <div className={styles.header}>
        <span className={styles.workspace}>{session.workspaceName}</span>
        <StatusBadge status={session.status} />
      </div>

      {/* Meta row */}
      <div className={styles.meta}>
        {session.isSubagent ? (
          <span className={styles.tag_subagent}>
            ⎇ {session.agentType ?? t("subagent")}
          </span>
        ) : (
          <span className={styles.tag_main}>◈ {t("main")}</span>
        )}
        {session.ideName && (
          <span className={styles.tag_ide}>{session.ideName}</span>
        )}
        {session.slug && (
          <span className={styles.slug}>{session.slug}</span>
        )}
      </div>

      {/* Preview */}
      {session.lastMessagePreview && (
        <p className={styles.preview}>{session.lastMessagePreview}</p>
      )}

      {/* Footer row */}
      <div className={styles.footer}>
        <TokenSpeed speed={session.tokenSpeed} />
        <span className={styles.tokens}>
          {session.totalOutputTokens.toLocaleString()} {t("tokens")}
        </span>
        <TimeAgo ms={session.lastActivityMs} />
      </div>
    </div>
  );
}
