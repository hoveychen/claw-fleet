import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import {
  INITIAL_TAIL,
  LOAD_EARLIER_STEP,
  useConnectionStore,
  useDetailStore,
  useSessionsStore,
  useUIStore,
} from "../store";
import { canResumeSession, canEnqueueSession } from "../types";
import type { DecisionHistoryRecord, LiveThinking, RawMessage, SessionInfo, TaskPlanDetail } from "../types";
import { AgentNavProvider } from "./AgentNavContext";
import { DecisionHistory } from "./DecisionHistory";
import { HandoffChainRow } from "./HandoffChainRow";
import { MessageList } from "./MessageList";
import type { PathLinkContext } from "../markdown/pathLinks";
import { ThinkingBlock } from "./blocks/ThinkingBlock";
import { ResumeComposer } from "./ResumeComposer";
import { ScratchpadView } from "./ScratchpadView";
import type { ExplorerEntry } from "./ExplorerPane";
import { SessionHeaderMenu } from "./SessionHeaderMenu";
import { SkillHistory } from "./SkillHistory";
import { TokenSpendPanel } from "./TokenSpendPanel";
import { CodexTokenPanel } from "./CodexTokenPanel";
import { WorkflowDag } from "./blocks/WorkflowDag";
import { useWorkflowTrees } from "../hooks/useWorkflowTrees";
import { isWorkflowAgent } from "../workflowAgent";
import { bgTaskIcon, bgTaskDataType } from "../bgTaskKinds";
import styles from "./SessionDetail.module.css";

const ACTIVE_STATUSES = new Set([
  "thinking", "executing", "streaming", "processing",
  "waitingInput", "active", "delegating",
]);

/** Max subagent tabs in the segmented strip. Active ones win the slots first,
 *  then the most-recently-active finished ones; the wrap scrolls past this. A
 *  parent that fanned out hundreds of subagents would otherwise flood the bar. */
const SUBAGENT_TAB_CAP = 12;

/** Standalone-mode live tail: re-pull the transcript tail at this cadence
 *  while the session is in an active status. */
const LIVE_TAIL_POLL_MS = 1500;

/** How close to the bottom still counts as "following the newest message". */
const FOLLOW_SLACK_PX = 200;

function shortId(id: string) {
  return id.slice(0, 8);
}

export function SessionDetail({
  lite = false,
  inline = false,
  sessionInfo = null,
  searchQuery: standaloneSearchQuery = null,
  paused = false,
}: {
  lite?: boolean;
  inline?: boolean;
  /** When set, the component runs in standalone mode: its own local
   *  session/messages state, independent from the global useDetailStore.
   *  Used by DecisionPanel's inline detail column and HistoryView so they
   *  don't fight the global detail store. Unlike global mode (backend watcher
   *  + `session-tail` push, single-slot), standalone mode live-tails by
   *  re-polling `get_messages_tail` while the session is active. */
  sessionInfo?: SessionInfo | null;
  /** Standalone mode only: highlight term forwarded to MessageList (e.g.
   *  the FTS query that matched this session in HistoryView). Ignored in
   *  global-store mode, which reads the query from useDetailStore. */
  searchQuery?: string | null;
  /** Standalone mode only: this instance is mounted but hidden (a background
   *  tab in HistoryView's tab bar). Kept alive so its messages, scroll and
   *  view-tab survive a tab switch, but every poller is frozen — otherwise
   *  each open tab of an active session would keep hitting `read_live_thinking`
   *  every 700ms and `get_messages_tail` every 1.5s in the background. Coming
   *  back to the foreground refetches the tail once, immediately. */
  paused?: boolean;
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
  // Render-synced mirrors for the live-tail interval callback, which must read
  // the latest values without re-arming the interval on every change.
  const localTailRef = useRef(localTail);
  localTailRef.current = localTail;
  const localLoadingRef = useRef(localLoading);
  localLoadingRef.current = localLoading;

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

  // Fetch the tail when localSession.jsonlPath changes — and again whenever a
  // background tab returns to the foreground, which is what makes `paused` a
  // dependency here. That second case is load-bearing: the live-tail poll below
  // only arms for *active* sessions, so a session that finished while its tab
  // was hidden would otherwise be left showing a stale transcript missing its
  // final messages. Refetching the *current* window (not INITIAL_TAIL) keeps
  // any earlier history the user had already loaded in this tab.
  useEffect(() => {
    if (!isStandalone || !localSession || paused) return;
    let cancelled = false;
    const tail = localTailRef.current;
    setLocalLoading(true);
    invoke<RawMessage[]>("get_messages_tail", {
      jsonlPath: localSession.jsonlPath,
      tail,
    })
      .then((msgs) => {
        if (cancelled) return;
        setLocalMessages(msgs);
        setLocalFullyLoaded(msgs.length < tail);
        setLocalLoading(false);
      })
      .catch(() => {
        if (cancelled) return;
        setLocalLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [isStandalone, localSession?.jsonlPath, paused]);

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
  const connection = useConnectionStore((s) => s.connection);
  const requestFileNav = useUIStore((s) => s.requestFileNav);
  const liveSession = useMemo(() => {
    if (!session) return null;
    return sessions.find((s) => s.id === session.id) ?? session;
  }, [session, sessions]);

  // Build tabs: [mainSession, ...activeSubagents]
  // Show tabs only when viewing a main agent that has active subagents,
  // or when viewing a subagent (show sibling tabs + parent).
  const scrollRef = useRef<HTMLDivElement>(null);
  const [isFollowing, setIsFollowing] = useState(true);
  type ViewTab =
    | "decisions"
    | "skills"
    | "messages"
    | "tokens"
    | "workflow"
    | "tasks"
    | "bgtasks"
    | "scratchpad";
  const [viewTab, setViewTab] = useState<ViewTab>("messages");
  const [decisionRecords, setDecisionRecords] = useState<DecisionHistoryRecord[]>([]);
  const [taskPlans, setTaskPlans] = useState<TaskPlanDetail[]>([]);
  const [liveThinking, setLiveThinking] = useState<LiveThinking | null>(null);

  // Claude Code Workflow runs for this session → reconstructed DAG, surfaced as
  // a session-level "Workflow" tab (not inline in the conversation). Publishing
  // the run rollup to the store drives the SessionCard chip.
  const setWorkflowRunCount = useSessionsStore((s) => s.setWorkflowRunCount);
  const workflowTrees = useWorkflowTrees(liveSession?.jsonlPath ?? null, !!liveSession, paused);
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
    // Backgrounded (hidden tab): freeze on the last reasoning we showed rather
    // than clearing it. This is the hottest poller in the component (700ms), so
    // leaving it running for every open tab is exactly what we're avoiding.
    if (paused) return;
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
  }, [liveSessionId, liveActive, paused]);

  // Standalone-mode live tail: the initial fetch above is a one-shot, which
  // was fine when the only standalone consumer was DecisionPanel (a pending
  // decision blocks the agent, so the transcript is static) — but HistoryView
  // renders *running* sessions this way. While the session is active, re-pull
  // the tail on an interval. `get_messages_tail` goes through the Backend
  // trait, so local and remote sessions both work. The status flip to
  // non-active lags the final transcript writes by a scan cycle, so the last
  // polls before the interval stops still catch the closing messages.
  // Frozen while backgrounded; the fetch effect above catches the tab up the
  // moment it returns to the foreground.
  const standaloneJsonlPath = isStandalone ? localSession?.jsonlPath : undefined;
  useEffect(() => {
    if (!standaloneJsonlPath || !liveActive || paused) return;
    let cancelled = false;
    let inFlight = false;
    const poll = () => {
      // Skip while a load-earlier refetch is in flight — it fetches a wider
      // window and would race a stale-tail overwrite from us.
      if (inFlight || localLoadingRef.current) return;
      inFlight = true;
      const tail = localTailRef.current;
      invoke<RawMessage[]>("get_messages_tail", {
        jsonlPath: standaloneJsonlPath,
        tail,
      })
        .then((msgs) => {
          if (cancelled) return;
          setLocalMessages(msgs);
          setLocalFullyLoaded(msgs.length < tail);
          // Window saturated: the next transcript write would slide already-
          // rendered messages out of the top. Grow the window so the visible
          // history stays anchored while the tail keeps extending.
          if (msgs.length >= tail) setLocalTail(tail + LOAD_EARLIER_STEP);
        })
        .catch(() => {})
        .finally(() => {
          inFlight = false;
        });
    };
    const timer = window.setInterval(poll, LIVE_TAIL_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [standaloneJsonlPath, liveActive, paused]);

  // Open a subagent's session — a workflow fan-out agent from the DAG, or a
  // Task subagent from its tool card. The scan registers both as
  // `agent-<agentId>` (see session.rs). No-op if a scan hasn't surfaced it yet.
  const openAgentSession = useCallback(
    (agentId: string) => {
      const target = sessions.find((s) => s.id === `agent-${agentId}`);
      if (target) open(target);
    },
    [sessions, open],
  );

  const agentNav = useMemo(
    () => ({
      open: openAgentSession,
      has: (agentId: string) => sessions.some((s) => s.id === `agent-${agentId}`),
    }),
    [openAgentSession, sessions],
  );

  useEffect(() => {
    setDecisionRecords([]);
  }, [liveSession?.id]);

  // Resume entry: only for "新会话"-launched main sessions (transcript
  // entrypoint tag) whose process has exited — spawns
  // `claude --resume <sid> -p <追问>` detached via the generic resume chain.
  // The form itself (prompt + attachments + model/effort/permission overrides)
  // lives in ResumeComposer, docked at the bottom of the 对话 tab whenever the
  // session is resumable — no separate "恢复会话" toggle to click.
  const canResume = !!liveSession && canResumeSession(liveSession);
  // While the turn is still running, the same dock offers to *queue* a
  // follow-up instead of resuming (which would race the live turn).
  const canEnqueue = !!liveSession && canEnqueueSession(liveSession);

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

  // TASKS.md plan for THIS session — scoped to the plan the session is focused
  // on (via its task-progress record), not every plan the workspace ever had.
  // Fetched through the Backend trait so it works for both local and remote
  // sessions. No session id → nothing to scope to → show nothing.
  const workspacePath = liveSession?.workspacePath;
  const sessionId = liveSession?.id;

  // Paths the agent wrote in backticks become clickable chips. Memoised because
  // MessageRow is memo'd — a fresh object each render would re-render every row.
  const pathLinks = useMemo<PathLinkContext | undefined>(() => {
    if (!workspacePath) return undefined;
    return {
      workspaceRoot: workspacePath,
      isLocal: connection?.type !== "remote",
      openInFiles: (absPath, line) => requestFileNav({ workspacePath, absPath, line }),
    };
  }, [workspacePath, connection?.type, requestFileNav]);

  useEffect(() => {
    if (!workspacePath || !sessionId) {
      setTaskPlans([]);
      return;
    }
    let cancelled = false;
    invoke<TaskPlanDetail[]>("get_task_plans", { workspacePath, sessionId })
      .then((r) => {
        if (!cancelled) setTaskPlans(r ?? []);
      })
      .catch(() => {
        if (!cancelled) setTaskPlans([]);
      });
    return () => {
      cancelled = true;
    };
  }, [workspacePath, sessionId]);
  const hasTaskPlans = taskPlans.length > 0;

  // Claude Code gives every session a private scratch dir and tells it to keep
  // temp files there rather than in /tmp. Probe the top level once per session:
  // an empty list (or an error — the session predates the convention, ran on
  // another machine, or never wrote a file) means no tab at all. Goes through
  // the Backend trait, so remote sessions resolve it on the probe host.
  const [scratchpadCount, setScratchpadCount] = useState(0);
  useEffect(() => {
    if (!workspacePath || !sessionId) {
      setScratchpadCount(0);
      return;
    }
    let cancelled = false;
    invoke<ExplorerEntry[]>("list_scratchpad_dir", {
      workspace: workspacePath,
      sessionId,
      relPath: "",
    })
      .then((entries) => {
        if (!cancelled) setScratchpadCount(entries?.length ?? 0);
      })
      .catch(() => {
        if (!cancelled) setScratchpadCount(0);
      });
    return () => {
      cancelled = true;
    };
  }, [workspacePath, sessionId]);
  const hasScratchpad = scratchpadCount > 0;

  // Background tasks the session was still waiting on when it last ended a turn
  // (shells, monitors, subagents — see `bg_guard`). Only populated while the
  // session's latest hook event is that Stop and within the 5-min freshness
  // window, so the tab naturally appears only when there's something to show.
  const bgTasks = liveSession?.backgroundTasks ?? [];
  const hasBgTasks = bgTasks.length > 0;

  // Switching to a session without a scratchpad would otherwise strand the view
  // on a tab whose button no longer renders. Same for the background-tasks tab,
  // whose array empties as soon as the session takes another turn.
  useEffect(() => {
    if (viewTab === "scratchpad" && !hasScratchpad) setViewTab("messages");
    if (viewTab === "bgtasks" && !hasBgTasks) setViewTab("messages");
  }, [viewTab, hasScratchpad, hasBgTasks]);

  const pickTab = useCallback((tab: ViewTab) => {
    setViewTab(tab);
  }, []);

  // `isFollowing` drives the footer, but the pin below runs from a
  // ResizeObserver callback that must not re-subscribe on every state change —
  // so mirror the flag into a ref and write both through one setter.
  const followingRef = useRef(true);
  const setFollowing = useCallback((next: boolean) => {
    followingRef.current = next;
    setIsFollowing(next);
  }, []);

  const checkFollow = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const dist = el.scrollHeight - el.scrollTop - el.clientHeight;
    setFollowing(dist < FOLLOW_SLACK_PX);
  }, [setFollowing]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.addEventListener("scroll", checkFollow, { passive: true });
    return () => el.removeEventListener("scroll", checkFollow);
  }, [checkFollow, session, viewTab]);

  // A different session, or a fresh visit to the tab, starts pinned again.
  useEffect(() => {
    setFollowing(true);
  }, [liveSession?.id, viewTab, setFollowing]);

  // Pin the viewport to the newest message for as long as the reader has not
  // scrolled away to read history.
  //
  // A transcript's height is not final on the frame it mounts: message images
  // carry no intrinsic size, and syntax highlighting and markdown settle over
  // later frames. Jumping to `scrollHeight` once inside a rAF — as this used to
  // — measures a container that is still nearly empty on a long transcript, so
  // the browser clamps scrollTop back to ~0; the old code then latched a
  // done-flag, and nothing ever corrected the position once the content grew.
  // That parked the reader at the top of several thousand px of messages.
  // Re-pin on every height change instead.
  const hasMessages = messages.length > 0;
  const hasLiveThinking = !!(liveThinking?.streaming && liveThinking.thinking);
  useEffect(() => {
    if (viewTab !== "messages") return;
    const el = scrollRef.current;
    if (!el) return;

    const pin = () => {
      if (!followingRef.current) return;
      el.scrollTop = el.scrollHeight;
    };

    // Observe the children, not the scroll box: the box's own border box never
    // changes size, it is the content inside it that grows. Re-subscribing when
    // the child set changes (loading placeholder -> list, live thinking block)
    // is why those two flags are in the dependency list.
    const ro = new ResizeObserver(pin);
    for (const child of Array.from(el.children)) ro.observe(child);
    pin();
    return () => ro.disconnect();
  }, [viewTab, liveSession?.id, hasMessages, hasLiveThinking]);

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

    if (subagents.length === 0) return [];

    // Show finished subagents too, not just the running one: once a subagent
    // goes Idle (or stalls at RateLimited) it used to vanish from this strip,
    // leaving the only way back into its transcript the "open subagent" button
    // buried in its Agent card. Order active first, then most-recently-active
    // finished ones, and cap the strip — a parent that fanned out dozens/hundreds
    // of subagents would otherwise overflow the tab bar. The wrap scrolls
    // horizontally (see .agent_seg_wrap) so the capped set stays reachable.
    const active = subagents.filter((s) => ACTIVE_STATUSES.has(s.status));
    const finished = subagents
      .filter((s) => !ACTIVE_STATUSES.has(s.status))
      .sort((a, b) => b.lastActivityMs - a.lastActivityMs);
    let ordered = [...active, ...finished].slice(0, SUBAGENT_TAB_CAP);

    // Never drop the subagent currently being viewed, even if more-recent
    // siblings pushed it past the cap — its tab must stay selectable.
    if (
      liveSession.isSubagent &&
      !ordered.some((s) => s.id === liveSession.id)
    ) {
      ordered = [liveSession, ...ordered.slice(0, SUBAGENT_TAB_CAP - 1)];
    }

    return mainSession ? [mainSession, ...ordered] : ordered;
  }, [liveSession, sessions]);

  return (
      <div className={`${styles.root} ${liveSession ? styles.open : ""} ${lite ? styles.lite : ""} ${inline ? styles.inline : ""}`}>
        {liveSession && (
          <>
          {/* Header — two rows. The AI title leads (it is what identifies the
              session); everything you only ever copy (session id, transcript
              path, workspace path) lives behind the ⋯ menu. */}
          <div className={styles.header}>
            <div className={styles.header_row}>
              <div
                className={styles.header_title}
                title={liveSession.aiTitle || liveSession.workspacePath}
              >
                {liveSession.aiTitle || liveSession.workspaceName}
              </div>
              <SessionHeaderMenu
                sessionId={liveSession.id}
                jsonlPath={liveSession.jsonlPath}
                workspacePath={liveSession.workspacePath}
                isLocal={connection?.type !== "remote"}
              />
              {!inline && (
                <button className={styles.close_btn} onClick={close} title={t("common.close") || "Close"}>
                  ✕
                </button>
              )}
            </div>
            <div className={styles.meta_row}>
              {/* Only when the title line isn't already the workspace name. */}
              {liveSession.aiTitle && (
                <span
                  className={styles.workspace_chip}
                  title={liveSession.workspacePath}
                >
                  {liveSession.workspaceName}
                </span>
              )}
              {liveSession.isSubagent ? (
                <span className={styles.tag_subagent}>
                  ⎇ {liveSession.agentType ?? t("subagent")}
                </span>
              ) : (
                <span className={styles.tag_main}>◈ {t("main")}</span>
              )}
              {liveSession.contextPercent != null && (
                <span
                  className={`${styles.meta_chip} ${liveSession.contextPercent >= 0.8 ? styles.meta_chip_warn : ""}`}
                  title={t("card.tip_context", { percent: Math.round(liveSession.contextPercent * 100) })}
                >
                  ctx {Math.round(liveSession.contextPercent * 100)}%
                </span>
              )}
              {(liveSession.totalCostUsd ?? 0) >= 0.005 && (
                <span className={styles.meta_chip} title={t("card.tip_cost")}>
                  ${liveSession.totalCostUsd.toFixed(2)}
                </span>
              )}
              <span className={styles.meta_chip} title={t("tokens_out")}>
                {liveSession.totalOutputTokens.toLocaleString()} tok
              </span>
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
              {liveSession.ideName && (
                <span className={styles.meta_chip}>{liveSession.ideName}</span>
              )}
              {liveSession.slug && (
                <span className={styles.slug} title={t("card.tip_slug", { slug: liveSession.slug })}>
                  {liveSession.slug}
                </span>
              )}
            </div>
            {/* Handoff relay chain — chip toggles the chain detail panel */}
            {liveSession.handoff && <HandoffChainRow session={liveSession} />}
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
            {hasBgTasks && (
              <button
                className={`${styles.view_tab} ${viewTab === "bgtasks" ? styles.view_tab_active : ""}`}
                onClick={() => pickTab("bgtasks")}
              >
                {t("detail.tab_bgtasks")} ({bgTasks.length})
              </button>
            )}
            {hasScratchpad && (
              <button
                className={`${styles.view_tab} ${viewTab === "scratchpad" ? styles.view_tab_active : ""}`}
                onClick={() => pickTab("scratchpad")}
              >
                {t("detail.tab_scratchpad")} ({scratchpadCount})
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

          {viewTab === "scratchpad" && workspacePath && sessionId && (
            <ScratchpadView workspace={workspacePath} sessionId={sessionId} />
          )}

          {viewTab === "decisions" && (
            <DecisionHistory records={decisionRecords} mode="tab" />
          )}

          {viewTab === "skills" && (
            <div className={styles.skills_panel}>
              <SkillHistory jsonlPath={liveSession.jsonlPath} mode="tab" />
            </div>
          )}

          {viewTab === "tokens" && liveSession && (
            liveSession.agentSource === "codex" ? (
              <CodexTokenPanel jsonlPath={liveSession.jsonlPath} />
            ) : (
              <TokenSpendPanel
                jsonlPath={liveSession.jsonlPath}
                workspacePath={liveSession.workspacePath}
              />
            )
          )}

          {viewTab === "tasks" && (
            <div className={styles.tasks_panel}>
              {taskPlans.map((plan, pi) => {
                const done = plan.items.filter((it) => it.done).length;
                const total = plan.items.length;
                const allDone = total > 0 && done === total;
                // Prefer the human-readable `**Plan:**` title; fall back to the
                // sentinel id, then to the anonymous label.
                const title = plan.title ?? plan.id ?? t("detail.tasks_anonymous");
                // Keep the id as a secondary tag only when a title is present —
                // otherwise the title already *is* the id, no need to repeat it.
                const showId = Boolean(plan.title && plan.id);
                // The first still-pending item is "current" for this plan —
                // the visible answer to "做到第几个 P 了".
                const currentIdx = plan.items.findIndex((it) => !it.done);
                return (
                  <div key={plan.id ?? `plan-${pi}`} className={styles.tasks_plan}>
                    <div className={styles.tasks_plan_head}>
                      <span className={styles.tasks_plan_title}>{title}</span>
                      <span
                        className={`${styles.tasks_plan_status} ${allDone ? styles.tasks_plan_status_done : styles.tasks_plan_status_active}`}
                      >
                        {allDone
                          ? t("detail.tasks_status_done")
                          : t("detail.tasks_status_active")}
                      </span>
                      <span className={styles.tasks_plan_count}>
                        {done}/{total}
                      </span>
                    </div>
                    {(showId || plan.source) && (
                      <div className={styles.tasks_plan_sub}>
                        {showId && <span className={styles.tasks_plan_id}>{plan.id}</span>}
                        {plan.source && (
                          <span className={styles.tasks_plan_source} title={plan.source}>
                            {plan.source}
                          </span>
                        )}
                      </div>
                    )}
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

          {viewTab === "bgtasks" && (
            <div className={styles.bgtasks_panel}>
              {bgTasks.map((bt) => {
                const icon = bgTaskIcon(bt.type);
                const dataType = bgTaskDataType(bt.type);
                const label = bt.description || bt.command || bt.id;
                // A subagent task correlates to its own scanned session
                // (`agent-<id>`, see session.rs / openAgentSession). When that
                // session is present we read its *live* status and let the row
                // open it — far more accurate than the last-Stop snapshot,
                // which for a subagent can be minutes stale.
                const linked =
                  bt.type === "subagent"
                    ? sessions.find((s) => s.id === `agent-${bt.id}`)
                    : undefined;
                if (linked) {
                  return (
                    <button
                      key={bt.id}
                      className={styles.bgtask_item_link}
                      onClick={() => openAgentSession(bt.id)}
                      title={t("detail.bgtask_open_hint")}
                    >
                      <span className={styles.bgtask_icon} aria-hidden>{icon}</span>
                      <span className={styles.tab_dot} data-status={linked.status} />
                      <span className={styles.bgtask_type} data-bgtype={dataType}>
                        {linked.agentType ?? bt.type}
                      </span>
                      <span className={styles.bgtask_desc}>
                        {linked.aiTitle || label}
                      </span>
                    </button>
                  );
                }
                // Shell / monitor, or a subagent whose session hasn't surfaced
                // (or already aged out): no live source, so this row reflects
                // the *last Stop* only — flag it as such rather than imply it's
                // current.
                return (
                  <div key={bt.id} className={styles.bgtask_item}>
                    <span className={styles.bgtask_icon} aria-hidden>{icon}</span>
                    <span className={styles.bgtask_type} data-bgtype={dataType}>
                      {bt.type}
                    </span>
                    <span className={styles.bgtask_desc}>{label}</span>
                    <span
                      className={styles.bgtask_stale}
                      title={
                        liveSession.lastActivityMs
                          ? new Date(liveSession.lastActivityMs).toLocaleString()
                          : undefined
                      }
                    >
                      {t("detail.bgtask_as_of_stop")}
                    </span>
                  </div>
                );
              })}
              <div className={styles.bgtasks_note}>{t("detail.bgtasks_note")}</div>
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
                {/* The "load earlier" control lives inside MessageList, which
                    owns the render window this button used to duplicate. */}
                <AgentNavProvider nav={agentNav}>
                  <MessageList
                    messages={messages}
                    isLoading={isLoading}
                    searchQuery={searchQuery}
                    status={liveSession?.status ?? null}
                    decisionRecords={decisionRecords}
                    onLoadEarlier={loadEarlier}
                    fullyLoaded={fullyLoaded}
                    isLoadingEarlier={isLoading && messages.length > 0}
                    paths={pathLinks}
                    jsonlPath={session?.jsonlPath}
                  />
                </AgentNavProvider>
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

              {/* Resume/enqueue composer, docked at the bottom of the
                  conversation. When the turn has ended it resumes; while the
                  turn is still running it queues a follow-up (delivered when the
                  turn ends). One of canResume / canEnqueue holds at a time. */}
              {(canResume || canEnqueue) && liveSession && (
                <div className={styles.resume_dock}>
                  <ResumeComposer
                    sessionId={liveSession.id}
                    workspacePath={liveSession.workspacePath}
                    agentSource={liveSession.agentSource}
                    onResumed={() => {}}
                    mode={canEnqueue ? "enqueue" : "resume"}
                    pendingMessages={liveSession.pendingMessages ?? []}
                  />
                </div>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}
