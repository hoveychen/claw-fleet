import { useTranslation } from "react-i18next";
import type { SessionInfo } from "../types";
import { RateLimitCountdown, StatusIcon, SubagentTypeIcon } from "./SessionCard";
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
  const title = session.aiTitle || session.workspaceName;
  const preview = session.lastMessagePreview?.trim() ?? "";
  const speed = session.tokenSpeed;
  const isSub = session.isSubagent;
  const extendThread = isSub && nextIsSubagent;

  return (
    <button
      className={`${styles.card} ${isSub ? styles.card_subagent : styles.card_main} ${extendThread ? styles.card_subagent_extend : ""}`}
      data-status={session.status}
      data-session-id={session.id}
      onClick={onClick}
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
          <span className={styles.badge_main} title={t("main")}>◈ {t("main")}</span>
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
          <RateLimitCountdown state={session.rateLimit} />
        </div>
      )}
    </button>
  );
}
