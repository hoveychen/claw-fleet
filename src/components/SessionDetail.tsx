import { useTranslation } from "react-i18next";
import { useDetailStore } from "../store";
import { MessageList } from "./MessageList";
import { SkillHistory } from "./SkillHistory";
import styles from "./SessionDetail.module.css";

export function SessionDetail() {
  const { t } = useTranslation();
  const { session, messages, isLoading, close } = useDetailStore();

  return (
      <div className={`${styles.root} ${session ? styles.open : ""}`}>
        {session && (
          <>
          {/* Header */}
          <div className={styles.header}>
            <div className={styles.header_row}>
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
                <button className={styles.close_btn} onClick={close}>
                  ✕
                </button>
              </div>
            </div>
            {session.aiTitle && (
              <div className={styles.ai_title}>{session.aiTitle}</div>
            )}
          </div>

          {/* Path */}
          <div className={styles.path}>{session.workspacePath}</div>

          {/* Skill history */}
          <SkillHistory jsonlPath={session.jsonlPath} />

          {/* Messages */}
          <div className={styles.scroll_area}>
            <MessageList messages={messages} isLoading={isLoading} />
          </div>
        </>
      )}
    </div>
  );
}
