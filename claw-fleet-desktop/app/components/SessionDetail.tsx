import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { useDetailStore, useSessionsStore } from "../store";
import type { DecisionHistoryRecord, SessionInfo } from "../types";
import { DecisionHistory } from "./DecisionHistory";
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

export function SessionDetail({ lite = false }: { lite?: boolean } = {}) {
  const { t } = useTranslation();
  const { session, messages, isLoading, searchQuery, fullyLoaded, close, open, loadEarlier } =
    useDetailStore();
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
  type ViewTab = "decisions" | "skills" | "messages";
  const [viewTab, setViewTab] = useState<ViewTab>("decisions");
  const [userPickedTab, setUserPickedTab] = useState(false);
  const [decisionRecords, setDecisionRecords] = useState<DecisionHistoryRecord[]>([]);
  const [decisionsLoaded, setDecisionsLoaded] = useState(false);

  useEffect(() => {
    setUserPickedTab(false);
    setDecisionsLoaded(false);
    setDecisionRecords([]);
  }, [liveSession?.id]);

  useEffect(() => {
    const sid = liveSession?.id;
    if (!sid) return;
    let cancelled = false;
    invoke<DecisionHistoryRecord[]>("list_session_decisions", {
      sessionId: sid,
      jsonlPath: liveSession?.jsonlPath ?? null,
    })
      .then((r) => {
        if (cancelled) return;
        setDecisionRecords(r ?? []);
        setDecisionsLoaded(true);
      })
      .catch(() => {
        if (cancelled) return;
        setDecisionRecords([]);
        setDecisionsLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, [liveSession?.id, liveSession?.jsonlPath]);

  useEffect(() => {
    if (!decisionsLoaded || userPickedTab) return;
    setViewTab(decisionRecords.length > 0 ? "decisions" : "messages");
  }, [decisionsLoaded, decisionRecords.length, userPickedTab]);

  const pickTab = useCallback((tab: ViewTab) => {
    setUserPickedTab(true);
    setViewTab(tab);
  }, []);

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
  }, [checkFollow, session, viewTab]);

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
      <div className={`${styles.root} ${liveSession ? styles.open : ""} ${lite ? styles.lite : ""}`}>
        {liveSession && (
          <>
          {/* Header */}
          <div className={styles.header}>
            <div className={styles.header_row}>
              <div className={styles.header_left}>
                <span className={styles.workspace} title={liveSession.workspaceName}>
                  {liveSession.workspaceName}
                </span>
                {liveSession.isSubagent ? (
                  <span className={styles.tag_subagent}>
                    ⎇ {liveSession.agentType ?? t("subagent")}
                  </span>
                ) : (
                  <span className={styles.tag_main}>◈ {t("main")}</span>
                )}
              </div>
              <button className={styles.close_btn} onClick={close} title={t("common.close") || "Close"}>
                ✕
              </button>
            </div>
            {liveSession.aiTitle && (
              <div className={styles.ai_title}>{liveSession.aiTitle}</div>
            )}
            <div className={styles.meta_row}>
              {liveSession.slug && (
                <span className={styles.slug}>{liveSession.slug}</span>
              )}
              {liveSession.ideName && (
                <span className={styles.ide}>{liveSession.ideName}</span>
              )}
              <span className={styles.tokens} title={t("tokens_out")}>
                {liveSession.totalOutputTokens.toLocaleString()} {t("tokens_out")}
              </span>
              {(liveSession.totalCostUsd ?? 0) >= 0.005 && (
                <span className={styles.cost} title={t("card.tip_cost")}>
                  ${liveSession.totalCostUsd.toFixed(2)}
                </span>
              )}
              {(liveSession.compactCount ?? 0) > 0 && (
                <span
                  className={styles.compact_chip}
                  title={t("card.tip_compact", {
                    count: liveSession.compactCount ?? 0,
                    pre: (liveSession.compactPreTokens ?? 0).toLocaleString(),
                    post: (liveSession.compactPostTokens ?? 0).toLocaleString(),
                    cost: (liveSession.compactCostUsd ?? 0).toFixed(2),
                  })}
                >
                  ⊞ {liveSession.compactCount}× ~${(liveSession.compactCostUsd ?? 0).toFixed(2)}
                </span>
              )}
              {liveSession.contextPercent != null && (
                <span
                  className={`${styles.context} ${liveSession.contextPercent >= 0.8 ? styles.context_high : ""}`}
                  title={t("card.tip_context", { percent: Math.round(liveSession.contextPercent * 100) })}
                >
                  ctx {Math.round(liveSession.contextPercent * 100)}%
                </span>
              )}
            </div>
          </div>

          {/* Path */}
          <div className={styles.path}>{liveSession.workspacePath}</div>

          {/* Combined tab bar: subagent tabs (if any) + view tabs */}
          <div className={styles.tab_bar}>
            {tabs.length > 0 && (
              <>
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
                <span className={styles.tab_separator} aria-hidden="true" />
              </>
            )}
            <button
              className={`${styles.view_tab} ${viewTab === "decisions" ? styles.view_tab_active : ""}`}
              onClick={() => pickTab("decisions")}
            >
              {t("detail.tab_decisions")}
            </button>
            <button
              className={`${styles.view_tab} ${viewTab === "skills" ? styles.view_tab_active : ""}`}
              onClick={() => pickTab("skills")}
            >
              {t("detail.tab_skills")}
            </button>
            <button
              className={`${styles.view_tab} ${viewTab === "messages" ? styles.view_tab_active : ""}`}
              onClick={() => pickTab("messages")}
            >
              {t("detail.tab_messages")}
            </button>
          </div>

          {viewTab === "decisions" && (
            <DecisionHistory records={decisionRecords} mode="tab" />
          )}

          {viewTab === "skills" && (
            <div className={styles.skills_panel}>
              <SkillHistory jsonlPath={liveSession.jsonlPath} mode="tab" />
            </div>
          )}

          {viewTab === "messages" && (
            <>
              <div ref={scrollRef} className={styles.scroll_area}>
                {!fullyLoaded && messages.length > 0 && (
                  <button
                    className={styles.load_earlier_btn}
                    onClick={loadEarlier}
                    disabled={isLoading}
                  >
                    {isLoading
                      ? t("detail.loading_earlier") || "Loading…"
                      : t("detail.load_earlier") || "Load earlier messages"}
                  </button>
                )}
                <MessageList messages={messages} isLoading={isLoading} searchQuery={searchQuery} />
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
        </>
      )}
    </div>
  );
}
