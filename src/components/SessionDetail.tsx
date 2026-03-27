import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDetailStore, useSessionsStore } from "../store";
import type { SessionInfo } from "../types";
import { MessageList } from "./MessageList";
import { SkillHistory } from "./SkillHistory";
import styles from "./SessionDetail.module.css";

const ACTIVE_STATUSES = new Set([
  "thinking", "executing", "streaming", "processing",
  "waitingInput", "active", "delegating",
]);

function shortId(id: string) {
  return id.slice(0, 8);
}

export function SessionDetail() {
  const { t } = useTranslation();
  const { session, messages, isLoading, close, open } = useDetailStore();
  const sessions = useSessionsStore((s) => s.sessions);
  const liveSession = useMemo(() => {
    if (!session) return null;
    return sessions.find((s) => s.id === session.id) ?? session;
  }, [session, sessions]);

  // Build tabs: [mainSession, ...activeSubagents]
  // Show tabs only when viewing a main agent that has active subagents,
  // or when viewing a subagent (show sibling tabs + parent).
  const scrollRef = useRef<HTMLDivElement>(null);
  const [isFollowing, setIsFollowing] = useState(true);

  const checkFollow = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const dist = el.scrollHeight - el.scrollTop - el.clientHeight;
    setIsFollowing(dist < 200);
  }, []);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.addEventListener("scroll", checkFollow, { passive: true });
    return () => el.removeEventListener("scroll", checkFollow);
  }, [checkFollow, session]);

  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, []);

  const tabs = useMemo((): SessionInfo[] => {
    if (!liveSession) return [];

    let mainSession: SessionInfo | undefined;
    let subagents: SessionInfo[];

    if (liveSession.isSubagent && liveSession.parentSessionId) {
      mainSession = sessions.find((s) => s.id === liveSession.parentSessionId);
      subagents = sessions.filter(
        (s) => s.isSubagent && s.parentSessionId === liveSession.parentSessionId
      );
    } else {
      mainSession = liveSession;
      subagents = sessions.filter(
        (s) => s.isSubagent && s.parentSessionId === liveSession.id
      );
    }

    const activeSubagents = subagents.filter((s) => ACTIVE_STATUSES.has(s.status));
    if (activeSubagents.length === 0) return [];

    return mainSession ? [mainSession, ...activeSubagents] : activeSubagents;
  }, [liveSession, sessions]);

  return (
      <div className={`${styles.root} ${liveSession ? styles.open : ""}`}>
        {liveSession && (
          <>
          {/* Header */}
          <div className={styles.header}>
            <div className={styles.header_row}>
              <div className={styles.header_left}>
                <span className={styles.workspace}>{liveSession.workspaceName}</span>
                {liveSession.isSubagent ? (
                  <span className={styles.tag_subagent}>
                    ⎇ {liveSession.agentType ?? t("subagent")}
                  </span>
                ) : (
                  <span className={styles.tag_main}>◈ {t("main")}</span>
                )}
                {liveSession.slug && (
                  <span className={styles.slug}>{liveSession.slug}</span>
                )}
              </div>
              <div className={styles.header_right}>
                {liveSession.ideName && (
                  <span className={styles.ide}>{liveSession.ideName}</span>
                )}
                <span className={styles.tokens}>
                  {liveSession.totalOutputTokens.toLocaleString()} {t("tokens_out")}
                </span>
                {liveSession.contextPercent != null && (
                  <span
                    className={`${styles.context} ${liveSession.contextPercent >= 0.8 ? styles.context_high : ""}`}
                    title={t("card.tip_context", { percent: Math.round(liveSession.contextPercent * 100) })}
                  >
                    ctx {Math.round(liveSession.contextPercent * 100)}%
                  </span>
                )}
                <button className={styles.close_btn} onClick={close}>
                  ✕
                </button>
              </div>
            </div>
            {liveSession.aiTitle && (
              <div className={styles.ai_title}>{liveSession.aiTitle}</div>
            )}
          </div>

          {/* Subagent tabs */}
          {tabs.length > 0 && (
            <div className={styles.tab_bar}>
              {tabs.map((tab) => (
                <button
                  key={tab.id}
                  className={`${styles.tab} ${tab.id === liveSession.id ? styles.tab_active : ""}`}
                  onClick={() => { if (tab.id !== liveSession.id) open(tab); }}
                >
                  <span
                    className={styles.tab_dot}
                    data-status={tab.status}
                  />
                  {tab.isSubagent
                    ? `⎇ ${tab.agentType ?? shortId(tab.id)}`
                    : `◈ ${t("main")}`}
                </button>
              ))}
            </div>
          )}

          {/* Path */}
          <div className={styles.path}>{liveSession.workspacePath}</div>

          {/* Skill history */}
          <SkillHistory jsonlPath={liveSession.jsonlPath} />

          {/* Messages */}
          <div ref={scrollRef} className={styles.scroll_area}>
            <MessageList messages={messages} isLoading={isLoading} />
          </div>

          {/* Auto-follow indicator */}
          {isFollowing ? (
            <div className={styles.follow_bar}>
              {t("detail.following")}
            </div>
          ) : (
            <button className={styles.follow_bar_btn} onClick={scrollToBottom}>
              ↓ {t("detail.scroll_to_latest")}
            </button>
          )}
        </>
      )}
    </div>
  );
}
