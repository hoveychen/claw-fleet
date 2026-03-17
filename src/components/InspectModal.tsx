import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useDetailStore } from "../store";
import type { SessionInfo } from "../types";
import { MessageList } from "./MessageList";
import styles from "./InspectModal.module.css";

interface Props {
  session: SessionInfo;
  onClose: () => void;
}

export function InspectModal({ session, onClose }: Props) {
  const { t } = useTranslation();
  const { messages, isLoading, open, close } = useDetailStore();

  useEffect(() => {
    open(session);
    return () => {
      close();
    };
  }, [session.jsonlPath]);

  // Close on Escape
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div className={styles.header}>
          <div className={styles.header_left}>
            <span className={styles.workspace}>{session.workspaceName}</span>
            {session.isSubagent ? (
              <span className={styles.tag_subagent}>
                ⎇ {session.agentType ?? t("subagent")}
              </span>
            ) : (
              <span className={styles.tag_main}>◈ {t("main")}</span>
            )}
            {session.slug && (
              <span className={styles.slug}>{session.slug}</span>
            )}
          </div>
          <div className={styles.header_right}>
            {session.ideName && (
              <span className={styles.ide}>{session.ideName}</span>
            )}
            <span className={styles.tokens}>
              {session.totalOutputTokens.toLocaleString()} {t("tokens_out")}
            </span>
            <button className={styles.close_btn} onClick={onClose}>
              ✕
            </button>
          </div>
        </div>

        {/* Path */}
        <div className={styles.path}>{session.workspacePath}</div>

        {/* Messages */}
        <div className={styles.scroll_area}>
          <MessageList messages={messages} isLoading={isLoading} />
        </div>
      </div>
    </div>
  );
}
