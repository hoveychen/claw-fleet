import { useTranslation } from "react-i18next";
import type { SessionInfo } from "../types";
import { RateLimitControls, StatusIcon, SubagentTypeIcon } from "./SessionCard";
import styles from "./LiteSessionCard.module.css";

export function LiteSessionCard({
  session,
  onClick,
  nextIsSubagent = false,
}: {
  session: SessionInfo;
  onClick?: () => void;
  nextIsSubagent?: boolean;
}) {
  const { t } = useTranslation();
  const isSub = session.isSubagent;
  // Main agents now surface workspaceName via a badge, so don't duplicate it as the fallback title.
  const title = isSub ? (session.aiTitle || session.workspaceName) : (session.aiTitle ?? "");
  const preview = session.lastMessagePreview?.trim() ?? "";
  const speed = session.tokenSpeed;
  const extendThread = isSub && nextIsSubagent;

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onClick?.();
    }
  };

  return (
    <div
      className={`${styles.card} ${isSub ? styles.card_subagent : styles.card_main} ${extendThread ? styles.card_subagent_extend : ""}`}
      data-status={session.status}
      data-session-id={session.id}
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={handleKeyDown}
    >
      <div className={styles.row}>
        <span className={styles.status_icon} data-status={session.status}>
          <StatusIcon status={session.status} />
        </span>
        {isSub ? (
          <span className={styles.badge_sub} title={session.agentType ?? t("subagent")}>
            <SubagentTypeIcon type={session.agentType} />
            <span className={styles.badge_sub_label}>
              {session.agentType ?? t("subagent")}
            </span>
          </span>
        ) : (
          <span className={styles.badge_main} title={session.workspaceName}>
            <span className={styles.badge_main_label}>{session.workspaceName}</span>
          </span>
        )}
        <span className={styles.title} title={title}>{title}</span>
        {speed >= 0.5 && (
          <span className={styles.speed}>
            {speed.toFixed(1)}
            <span className={styles.speed_unit}> {t("tok_s")}</span>
          </span>
        )}
      </div>
      {preview && <div className={styles.preview}>{preview}</div>}
      {session.status === "rateLimited" && session.rateLimit && (
        <div className={styles.rate_limit}>
          <RateLimitControls session={session} />
        </div>
      )}
    </div>
  );
}
