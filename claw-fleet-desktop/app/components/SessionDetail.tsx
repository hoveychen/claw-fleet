import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { INITIAL_TAIL, LOAD_EARLIER_STEP, useDetailStore, useSessionsStore } from "../store";
import { NEW_SESSION_ENTRYPOINT } from "../types";
import type { DecisionHistoryRecord, LiveThinking, RawMessage, SessionInfo, TaskPlanDetail } from "../types";
import { DecisionHistory } from "./DecisionHistory";
import { MessageList } from "./MessageList";
import { ThinkingBlock } from "./blocks/ThinkingBlock";
import { SkillHistory } from "./SkillHistory";
import { TokenSpendPanel } from "./TokenSpendPanel";
import { WorkflowDag } from "./blocks/WorkflowDag";
import { useWorkflowTrees } from "../hooks/useWorkflowTrees";
import { isWorkflowAgent } from "../workflowAgent";
import styles from "./SessionDetail.module.css";

const ACTIVE_STATUSES = new Set([
  "thinking", "executing", "streaming", "processing",
  "waitingInput", "active", "delegating",
]);

function shortId(id: string) {
  return id.slice(0, 8);
}

export function SessionDetail({
  lite = false,
  inline = false,
  sessionInfo = null,
  searchQuery: standaloneSearchQuery = null,
}: {
  lite?: boolean;
  inline?: boolean;
  /** When set, the component runs in standalone mode: its own local
   *  session/messages state, independent from the global useDetailStore.
   *  Used by DecisionPanel's inline detail column so it doesn't fight the
   *  global detail store. No live tail in this mode —
   *  messages are a snapshot (pending decisions block the agent anyway). */
  sessionInfo?: SessionInfo | null;
  /** Standalone mode only: highlight term forwarded to MessageList (e.g.
   *  the FTS query that matched this session in HistoryView). Ignored in
   *  global-store mode, which reads the query from useDetailStore. */
  searchQuery?: string | null;
} = {}) {
  const { t } = useTranslation();
  const isStandalone = sessionInfo != null;
  const global = useDetailStore();

  // Local mirror state for standalone mode.
  const [localSession, setLocalSession] = useState<SessionInfo | null>(sessionInfo ?? null);
  const [localMessages, setLocalMessages] = useState<RawMessage[]>([]);
  const [localLoading, setLocalLoading] = useState(false);
  const [localFullyLoaded, setLocalFullyLoaded] = useState(false);
  const [localTail, setLocalTail] = useState<number>(INITIAL_TAIL);

  // External sessionInfo prop changed → reset local state and refetch.
  useEffect(() => {
    if (!isStandalone) return;
    if (sessionInfo && sessionInfo.id !== localSession?.id) {
      setLocalSession(sessionInfo);
      setLocalMessages([]);
      setLocalTail(INITIAL_TAIL);
      setLocalFullyLoaded(false);
    }
  }, [isStandalone, sessionInfo?.id]);

  // Fetch initial tail when localSession.jsonlPath changes.
  useEffect(() => {
    if (!isStandalone || !localSession) return;
    let cancelled = false;
    setLocalLoading(true);
    invoke<RawMessage[]>("get_messages_tail", {
      jsonlPath: localSession.jsonlPath,
      tail: INITIAL_TAIL,
    })
      .then((msgs) => {
        if (cancelled) return;
        setLocalMessages(msgs);
        setLocalFullyLoaded(msgs.length < INITIAL_TAIL);
        setLocalLoading(false);
      })
      .catch(() => {
        if (cancelled) return;
        setLocalLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [isStandalone, localSession?.jsonlPath]);

  const standaloneLoadEarlier = useCallback(async () => {
    if (!isStandalone || !localSession || localFullyLoaded) return;
    const nextTail = localTail + LOAD_EARLIER_STEP;
    setLocalLoading(true);
    setLocalTail(nextTail);
    try {
      const msgs = await invoke<RawMessage[]>("get_messages_tail", {
        jsonlPath: localSession.jsonlPath,
        tail: nextTail,
      });
      setLocalMessages(msgs);
      setLocalFullyLoaded(msgs.length < nextTail);
    } finally {
      setLocalLoading(false);
    }
  }, [isStandalone, localSession?.jsonlPath, localTail, localFullyLoaded]);

  // Resolved values — flip between local mirror and global store based on mode.
  const session = isStandalone ? localSession : global.session;
  const messages = isStandalone ? localMessages : global.messages;
  const isLoading = isStandalone ? localLoading : global.isLoading;
  const fullyLoaded = isStandalone ? localFullyLoaded : global.fullyLoaded;
  const searchQuery = isStandalone ? standaloneSearchQuery : global.searchQuery;
  const close = global.close;
  const open = useCallback(
    (s: SessionInfo) => {
      if (isStandalone) {
        setLocalSession(s);
        setLocalMessages([]);
        setLocalTail(INITIAL_TAIL);
        setLocalFullyLoaded(false);
      } else {
        global.open(s);
      }
    },
    [isStandalone, global.open],
  );
  const loadEarlier = isStandalone ? standaloneLoadEarlier : global.loadEarlier;

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
  type ViewTab = "decisions" | "skills" | "messages" | "tokens" | "workflow" | "tasks";
  const [viewTab, setViewTab] = useState<ViewTab>("messages");
  const [decisionRecords, setDecisionRecords] = useState<DecisionHistoryRecord[]>([]);
  const [taskPlans, setTaskPlans] = useState<TaskPlanDetail[]>([]);
  const [liveThinking, setLiveThinking] = useState<LiveThinking | null>(null);

  // Claude Code Workflow runs for this session → reconstructed DAG, surfaced as
  // a session-level "Workflow" tab (not inline in the conversation). Publishing
  // the run rollup to the store drives the SessionCard chip.
  const setWorkflowRunCount = useSessionsStore((s) => s.setWorkflowRunCount);
  const workflowTrees = useWorkflowTrees(liveSession?.jsonlPath ?? null, !!liveSession);
  const hasWorkflows = workflowTrees.length > 0;
  useEffect(() => {
    const sid = liveSession?.id;
    if (!sid) return;
    const running = workflowTrees.filter((tr) =>
      tr.agents.some((a) => a.status === "running"),
    ).length;
    setWorkflowRunCount(sid, { total: workflowTrees.length, running });
  }, [liveSession?.id, workflowTrees, setWorkflowRunCount]);
  // Live thinking: while the session is actively streaming, poll its
  // stream-json sidecar for the token-level reasoning the CLI is emitting right
  // now (the JSONL transcript only lands a *completed* thinking block, so this
  // is the only way to show reasoning as it streams — same payload the VS Code
  // extension renders, just teed to a file). Stops polling once the session
  // leaves an active status; clears when the stream is no longer streaming.
  const liveSessionId = liveSession?.id;
  const liveActive = !!liveSession && ACTIVE_STATUSES.has(liveSession.status);
  useEffect(() => {
    if (!liveSessionId || !liveActive) {
      setLiveThinking(null);
      return;
    }
    let cancelled = false;
    const poll = () => {
      invoke<LiveThinking | null>("read_live_thinking", { sessionId: liveSessionId })
        .then((lt) => {
          if (!cancelled) setLiveThinking(lt);
        })
        .catch(() => {
          if (!cancelled) setLiveThinking(null);
        });
    };
    poll();
    const timer = window.setInterval(poll, 700);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [liveSessionId, liveActive]);

  // Open a workflow fan-out agent's session (the scan registers it as
  // `agent-<agentId>`, see session.rs). No-op if a scan hasn't surfaced it yet.
  const openAgentSession = useCallback(
    (agentId: string) => {
      const target = sessions.find((s) => s.id === `agent-${agentId}`);
      if (target) open(target);
    },
    [sessions, open],
  );

  useEffect(() => {
    setDecisionRecords([]);
  }, [liveSession?.id]);

  // Resume entry: only for "新会话"-launched main sessions (transcript
  // entrypoint tag) that are not currently running — spawns
  // `claude --resume <sid> -p <追问>` detached via the generic resume chain.
  const [showResume, setShowResume] = useState(false);
  const [resumeText, setResumeText] = useState("");
  const [resuming, setResuming] = useState(false);
  const [resumeError, setResumeError] = useState<string | null>(null);
  useEffect(() => {
    setShowResume(false);
    setResumeText("");
    setResumeError(null);
  }, [liveSession?.id]);
  const canResume =
    !!liveSession &&
    !liveSession.isSubagent &&
    !ACTIVE_STATUSES.has(liveSession.status) &&
    liveSession.entrypoint === NEW_SESSION_ENTRYPOINT;
  const submitResume = useCallback(async () => {
    if (!liveSession) return;
    const prompt = resumeText.trim();
    if (!prompt || resuming) return;
    setResuming(true);
    setResumeError(null);
    try {
      await invoke("resume_rate_limited_session", {
        sessionId: liveSession.id,
        workspacePath: liveSession.workspacePath,
        prompt,
      });
      setShowResume(false);
      setResumeText("");
    } catch (e) {
      setResumeError(String((e as { message?: string })?.message ?? e));
    } finally {
      setResuming(false);
    }
  }, [liveSession?.id, liveSession?.workspacePath, resumeText, resuming]);

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
      })
      .catch(() => {
        if (cancelled) return;
        setDecisionRecords([]);
      });
    return () => {
      cancelled = true;
    };
  }, [liveSession?.id, liveSession?.jsonlPath]);

  // Honor an explicit initial tab (e.g. the user clicked the card's plan row →
  // open straight to "tasks"). Default tab is "messages" (对话).
  useEffect(() => {
    if (isStandalone) return;
    const tab = global.initialTab;
    if (!tab) return;
    setViewTab(tab as ViewTab);
  }, [isStandalone, global.session?.id, global.initialTab]);

  // TASKS.md plans for this session's workspace (full task-item list). Fetched
  // through the Backend trait so it works for both local and remote sessions.
  const workspacePath = liveSession?.workspacePath;
  useEffect(() => {
    if (!workspacePath) {
      setTaskPlans([]);
      return;
    }
    let cancelled = false;
    invoke<TaskPlanDetail[]>("get_task_plans", { workspacePath })
      .then((r) => {
        if (!cancelled) setTaskPlans(r ?? []);
      })
      .catch(() => {
        if (!cancelled) setTaskPlans([]);
      });
    return () => {
      cancelled = true;
    };
  }, [workspacePath]);
  const hasTaskPlans = taskPlans.length > 0;

  const pickTab = useCallback((tab: ViewTab) => {
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

  // Auto-scroll bookkeeping for the messages tab.
  const initialScrollDoneRef = useRef(false);
  const firstMessageKeyRef = useRef<string | null>(null);
  const lastMessageKeyRef = useRef<string | null>(null);

  useEffect(() => {
    initialScrollDoneRef.current = false;
    firstMessageKeyRef.current = null;
    lastMessageKeyRef.current = null;
  }, [liveSession?.id, viewTab]);

  // First mount with content: jump to bottom and sync isFollowing with reality
  // (the initial `true` default is wrong when the freshly mounted scroll area
  // already overflows but no user-scroll has fired yet).
  useEffect(() => {
    if (viewTab !== "messages") return;
    if (initialScrollDoneRef.current) return;
    if (messages.length === 0) return;
    if (!scrollRef.current) return;
    const id = requestAnimationFrame(() => {
      const e = scrollRef.current;
      if (!e) return;
      e.scrollTop = e.scrollHeight;
      initialScrollDoneRef.current = true;
      checkFollow();
    });
    return () => cancelAnimationFrame(id);
  }, [viewTab, messages.length, checkFollow]);

  // Live tail: on new trailing messages (not load-earlier), follow if user is
  // already at the bottom. Skip when the first message changed — that means
  // load-earlier prepended history and we must preserve the read position.
  useEffect(() => {
    if (viewTab !== "messages") return;
    if (!initialScrollDoneRef.current) return;
    if (messages.length === 0) {
      firstMessageKeyRef.current = null;
      lastMessageKeyRef.current = null;
      return;
    }
    const firstKey = messages[0]?.uuid ?? messages[0]?.timestamp ?? null;
    const last = messages[messages.length - 1];
    const lastKey = last?.uuid ?? last?.timestamp ?? null;
    const prevFirst = firstMessageKeyRef.current;
    const prevLast = lastMessageKeyRef.current;
    firstMessageKeyRef.current = firstKey;
    lastMessageKeyRef.current = lastKey;
    if (firstKey !== prevFirst) return;
    if (lastKey === prevLast) return;
    if (!isFollowing) return;
    const el = scrollRef.current;
    if (!el) return;
    requestAnimationFrame(() => {
      const e = scrollRef.current;
      if (!e) return;
      e.scrollTop = e.scrollHeight;
    });
  }, [messages, viewTab, isFollowing]);

  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, []);

  const tabs = useMemo((): SessionInfo[] => {
    if (!liveSession) return [];

    let mainSession: SessionInfo | undefined;
    let subagents: SessionInfo[];

    // Exclude workflow fan-out agents — surfaced only for "open from DAG node",
    // not as conversation tabs (100+ per run). See isWorkflowAgent.
    if (liveSession.isSubagent && liveSession.parentSessionId) {
      mainSession = sessions.find((s) => s.id === liveSession.parentSessionId);
      subagents = sessions.filter(
        (s) =>
          s.isSubagent &&
          s.parentSessionId === liveSession.parentSessionId &&
          !isWorkflowAgent(s)
      );
    } else {
      mainSession = liveSession;
      subagents = sessions.filter(
        (s) =>
          s.isSubagent &&
          s.parentSessionId === liveSession.id &&
          !isWorkflowAgent(s)
      );
    }

    const activeSubagents = subagents.filter((s) => ACTIVE_STATUSES.has(s.status));
    if (activeSubagents.length === 0) return [];

    return mainSession ? [mainSession, ...activeSubagents] : activeSubagents;
  }, [liveSession, sessions]);

  return (
      <div className={`${styles.root} ${liveSession ? styles.open : ""} ${lite ? styles.lite : ""} ${inline ? styles.inline : ""}`}>
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
              {canResume && (
                <button
                  className={`${styles.resume_btn} ${showResume ? styles.resume_btn_active : ""}`}
                  onClick={() => setShowResume((v) => !v)}
                  title={t("history.resume", "恢复会话")}
                >
                  ↻ {t("history.resume", "恢复会话")}
                </button>
              )}
              {!inline && (
                <button className={styles.close_btn} onClick={close} title={t("common.close") || "Close"}>
                  ✕
                </button>
              )}
            </div>
            {canResume && showResume && (
              <div className={styles.resume_form}>
                <textarea
                  className={styles.resume_input}
                  value={resumeText}
                  onChange={(e) => setResumeText(e.target.value)}
                  placeholder={t("history.resume_placeholder", "输入追问提示词后恢复会话…")}
                  rows={2}
                  autoFocus
                />
                <button
                  className={styles.resume_send}
                  disabled={!resumeText.trim() || resuming}
                  onClick={submitResume}
                >
                  {resuming ? t("history.resuming", "恢复中…") : t("history.resume", "恢复会话")}
                </button>
                {resumeError && <div className={styles.resume_error}>{resumeError}</div>}
              </div>
            )}
            {liveSession.aiTitle && (
              <div className={styles.ai_title}>{liveSession.aiTitle}</div>
            )}
            <div className={styles.meta_row}>
              {liveSession.slug && (
                <span className={styles.slug}>{liveSession.slug}</span>
              )}
              {liveSession.ideName && (
                <span className={styles.meta_chip}>{liveSession.ideName}</span>
              )}
              <span className={styles.meta_chip} title={t("tokens_out")}>
                {liveSession.totalOutputTokens.toLocaleString()} {t("tokens_out")}
              </span>
              {(liveSession.totalCostUsd ?? 0) >= 0.005 && (
                <span className={styles.meta_chip} title={t("card.tip_cost")}>
                  ${liveSession.totalCostUsd.toFixed(2)}
                </span>
              )}
              {(liveSession.compactCount ?? 0) > 0 && (
                <span
                  className={styles.meta_chip}
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
                  className={`${styles.meta_chip} ${liveSession.contextPercent >= 0.8 ? styles.meta_chip_warn : ""}`}
                  title={t("card.tip_context", { percent: Math.round(liveSession.contextPercent * 100) })}
                >
                  ctx {Math.round(liveSession.contextPercent * 100)}%
                </span>
              )}
            </div>
            <div className={styles.path} title={liveSession.workspacePath}>{liveSession.workspacePath}</div>
          </div>

          {/* Combined tab bar: subagent segmented control (if any) + view tabs */}
          <div className={styles.tab_bar}>
            {tabs.length > 0 && (
              <div className={styles.agent_seg_wrap}>
                {tabs.map((tab) => (
                  <button
                    key={tab.id}
                    className={`${styles.agent_seg} ${tab.id === liveSession.id ? styles.agent_seg_active : ""}`}
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
            <button
              className={`${styles.view_tab} ${viewTab === "messages" ? styles.view_tab_active : ""}`}
              onClick={() => pickTab("messages")}
            >
              {t("detail.tab_messages")}
            </button>
            <button
              className={`${styles.view_tab} ${viewTab === "skills" ? styles.view_tab_active : ""}`}
              onClick={() => pickTab("skills")}
            >
              {t("detail.tab_skills")}
            </button>
            <button
              className={`${styles.view_tab} ${viewTab === "decisions" ? styles.view_tab_active : ""}`}
              onClick={() => pickTab("decisions")}
            >
              {t("detail.tab_decisions")}
            </button>
            <button
              className={`${styles.view_tab} ${viewTab === "tokens" ? styles.view_tab_active : ""}`}
              onClick={() => pickTab("tokens")}
            >
              {t("detail.tab_tokens")}
            </button>
            {hasTaskPlans && (
              <button
                className={`${styles.view_tab} ${viewTab === "tasks" ? styles.view_tab_active : ""}`}
                onClick={() => pickTab("tasks")}
              >
                {t("detail.tab_tasks")}
              </button>
            )}
            {hasWorkflows && (
              <button
                className={`${styles.view_tab} ${viewTab === "workflow" ? styles.view_tab_active : ""}`}
                onClick={() => pickTab("workflow")}
              >
                {t("detail.tab_workflow")} ({workflowTrees.length})
              </button>
            )}
          </div>

          {viewTab === "decisions" && (
            <DecisionHistory records={decisionRecords} mode="tab" />
          )}

          {viewTab === "skills" && (
            <div className={styles.skills_panel}>
              <SkillHistory jsonlPath={liveSession.jsonlPath} mode="tab" />
            </div>
          )}

          {viewTab === "tokens" && liveSession && (
            <TokenSpendPanel
              jsonlPath={liveSession.jsonlPath}
              workspacePath={liveSession.workspacePath}
            />
          )}

          {viewTab === "tasks" && (
            <div className={styles.tasks_panel}>
              {taskPlans.map((plan, pi) => {
                const done = plan.items.filter((it) => it.done).length;
                // The first still-pending item is "current" for this plan —
                // the visible answer to "做到第几个 P 了".
                const currentIdx = plan.items.findIndex((it) => !it.done);
                return (
                  <div key={plan.id ?? `plan-${pi}`} className={styles.tasks_plan}>
                    <div className={styles.tasks_plan_head}>
                      <span className={styles.tasks_plan_id}>
                        {plan.id ?? t("detail.tasks_anonymous")}
                      </span>
                      <span className={styles.tasks_plan_count}>
                        {done}/{plan.items.length}
                      </span>
                      {plan.source && (
                        <span className={styles.tasks_plan_source} title={plan.source}>
                          {plan.source}
                        </span>
                      )}
                    </div>
                    <ul className={styles.tasks_items}>
                      {plan.items.map((it, ii) => {
                        const isCurrent = ii === currentIdx;
                        return (
                          <li
                            key={ii}
                            className={`${styles.tasks_item} ${it.done ? styles.tasks_item_done : ""} ${isCurrent ? styles.tasks_item_current : ""}`}
                          >
                            <span className={styles.tasks_check} aria-hidden>
                              {it.done ? "☑" : isCurrent ? "▶" : "☐"}
                            </span>
                            <span className={styles.tasks_text}>{it.text}</span>
                          </li>
                        );
                      })}
                    </ul>
                  </div>
                );
              })}
            </div>
          )}

          {viewTab === "workflow" && (
            <div className={styles.workflow_panel}>
              {workflowTrees.map((tree) => {
                const done = tree.agents.filter((a) => a.status === "done").length;
                return (
                  <div key={tree.runId} className={styles.workflow_run}>
                    <div className={styles.workflow_run_head}>
                      <span className={styles.workflow_run_name}>
                        {tree.name ?? tree.runId}
                      </span>
                      <span className={styles.workflow_run_meta}>
                        {tree.runId} · {done}/{tree.agents.length} agents
                      </span>
                    </div>
                    <WorkflowDag tree={tree} onOpenAgent={openAgentSession} />
                  </div>
                );
              })}
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
                {liveThinking?.streaming && liveThinking.thinking && (
                  <ThinkingBlock thinking={liveThinking.thinking} live />
                )}
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
