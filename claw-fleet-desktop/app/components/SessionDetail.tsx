import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useTranslation } from "react-i18next";
import {
  INITIAL_TAIL,
  LOAD_EARLIER_STEP,
  useConnectionStore,
  useDecisionStore,
  useDetailStore,
  useSessionsStore,
  useUIStore,
} from "../store";
import { CalendarClock, LoaderCircle, PanelRight } from "lucide-react";
import { canResumeSession, canEnqueueSession, preferredSessionTitle, shouldFollowSession, LIVE_STATUSES, SCHEDULE_ENTRYPOINT } from "../types";
import type { DecisionHistoryRecord, LiveThinking, RawMessage, SessionInfo, TaskPlanDetail } from "../types";
import { isRenderableRow, messageToText } from "../messageRows";
import { reconcileMessages } from "../messageReuse";
import { arrivedSince, nextLiveTail, recordId } from "../liveTailWindow";
import { withStallWatch } from "../loadDeadline";
import {
  initialFollowState,
  nextFollowState,
  type FollowInput,
  type FollowState,
} from "../followState";
import { nestedScrollerWillConsume } from "../nestedScroll";
import { currentViewMetrics, formatSnapshot, takeScrollSnapshot } from "../scrollSnapshot";
import { AgentNavProvider } from "./AgentNavContext";
import { HandoffChainRow } from "./HandoffChainRow";
import { PlanProgressRow } from "./PlanProgressRow";
import { WatchStatusRow } from "./WatchStatusRow";
import { MessageList } from "./MessageList";
import type { PathLinkContext } from "../markdown/pathLinks";
import type { WikiLinkContext } from "../markdown/wikiLinks";
import { WikiLinksProvider } from "../markdown/wikiLinksContext";
import { WebLinkProvider } from "../markdown/webLinks";
import { useWikiDocs } from "../hooks/useWikiDocs";
import { ResumeComposer } from "./ResumeComposer";
import type { ExplorerEntry } from "./ExplorerPane";
import { SessionHeaderMenu } from "./SessionHeaderMenu";
import { AgentScopeSwitcher } from "./AgentScopeSwitcher";
import { effortChipLabel, effortTitle, formatModel } from "./SessionCard";
import { inlineCodexFleetAsk, withCodexDecisionHistory } from "./codexDecision";
import { useWorkflowTrees } from "../hooks/useWorkflowTrees";
import { isWorkflowAgent } from "../workflowAgent";
import { subscribeDecisionHistoryRefresh } from "../decisionHistoryRefresh";
import {
  activeAuxTab,
  AGENTS_TAB,
  closeAux,
  closeDoc,
  initialAux,
  isAuxFacet,
  openDoc,
  pruneTab,
  showTab,
  syncLiveAgents,
  toggleTab,
  type AuxDocKind,
  type AuxState,
} from "../detailAux";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { SessionAuxPanel, type AuxTab } from "./SessionAuxPanel";
import { SessionFacetPanel } from "./SessionFacetPanel";
import { SubagentLiveCards } from "./SubagentLiveCards";
import { SessionAuxDoc } from "./SessionAuxDoc";
import styles from "./SessionDetail.module.css";
import { showLatestSync } from "../conversationPlaceholder";


/** Max subagents listed in the scope dropdown (AgentScopeSwitcher). Active ones
 *  win the slots first, then the most-recently-active finished ones. A parent
 *  that fanned out hundreds of subagents would otherwise flood the menu. */
const SUBAGENT_TAB_CAP = 12;

/** Narrower than this and the pane can't hold two readable columns, so the
 *  auxiliary panel floats over the conversation instead of splitting it. A
 *  4-way split of the 任务 page bottoms out at MIN_GROUP_PX (280), and
 *  DecisionPanel's inline column is narrower still. */
const AUX_OVERLAY_PX = 640;

/** Standalone-mode live tail: re-pull the transcript tail at this cadence
 *  while the session is in an active status. */
const LIVE_TAIL_POLL_MS = 1500;

/** After a resume/enqueue submit, keep the live-tail + live-thinking pollers
 *  armed for this long even though the session is still flipping to `live` via
 *  the next rescan. Without this the user's optimistic bubble would just sit
 *  there while nothing polls the JSONL, so the real transcript (and the reply)
 *  would only appear once rescan翻转 status — the exact干等 we're removing. */
const RESUME_GRACE_MS = 30_000;

/** A pending follow-up the user just submitted, shown immediately as a user
 *  bubble while `claude --resume` cold-starts and writes it into the JSONL.
 *  Removed once the real transcript row with the same text lands. */
interface OptimisticSend {
  id: string;
  text: string;
}

/** Build a synthetic `user` RawMessage from an optimistic send so it flows
 *  through the normal MessageList renderer (bubble layout, scroll-to-bottom,
 *  day separators). Passes `isRenderableRow` because it carries a text block. */
function optimisticToMessage(o: OptimisticSend): RawMessage {
  return {
    type: "user",
    uuid: o.id,
    timestamp: new Date().toISOString(),
    message: { role: "user", content: [{ type: "text", text: o.text }] },
  };
}

// DecisionPanel already embeds SessionDetail for its history sidecar. Keep the
// reverse dependency lazy so projecting a pending Codex card into the dialogue
// does not create an eager ESM cycle between the two modules.
const InlineFleetAskCard = lazy(() =>
  import("./DecisionPanel").then(({ FleetAskCard }) => ({ default: FleetAskCard })),
);

/** "由计划 X 触发" provenance chip. Shown only for sessions a one-shot schedule
 *  fired (entrypoint === SCHEDULE_ENTRYPOINT); reverse-maps the session id back
 *  to the schedule via list_schedules' firedSessionId to name the id. Clicking
 *  jumps to the Schedule page. Falls back to a generic "定时触发" label if the
 *  schedule record was cancelled/forgotten and no id is recoverable. */
function ScheduleProvenanceChip({ session }: { session: SessionInfo }) {
  const { t } = useTranslation();
  const setViewMode = useUIStore((s) => s.setViewMode);
  const [scheduleId, setScheduleId] = useState<string | null>(null);
  const isScheduled = session.entrypoint === SCHEDULE_ENTRYPOINT;
  useEffect(() => {
    if (!isScheduled) return;
    let alive = true;
    invoke<{ id: string; firedSessionId?: string }[]>("list_schedules")
      .then((list) => {
        if (!alive) return;
        const hit = list.find((s) => s.firedSessionId === session.id);
        if (hit) setScheduleId(hit.id);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [isScheduled, session.id]);
  if (!isScheduled) return null;
  return (
    <button
      className={styles.schedule_chip}
      title={t("detail.triggered_by_schedule_tip", "由 Fleet 定时任务触发,点击查看计划")}
      onClick={() => setViewMode("schedule")}
    >
      <CalendarClock size={11} strokeWidth={2.2} />
      {scheduleId
        ? t("detail.triggered_by_schedule", { id: scheduleId })
        : t("detail.triggered_by_schedule_generic")}
    </button>
  );
}

export function SessionDetail({
  inline = false,
  sessionInfo = null,
  searchQuery: standaloneSearchQuery = null,
  paused = false,
}: {
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
  const [localLoadingEarlier, setLocalLoadingEarlier] = useState(false);
  const [localStalled, setLocalStalled] = useState(false);
  /** Bumped by the stalled pane's retry button to re-run the fetch effect. */
  const [reloadKey, setReloadKey] = useState(0);
  const [localFullyLoaded, setLocalFullyLoaded] = useState(false);
  const [localTail, setLocalTail] = useState<number>(INITIAL_TAIL);
  // Render-synced mirrors for the live-tail interval callback, which must read
  // the latest values without re-arming the interval on every change.
  const localTailRef = useRef(localTail);
  localTailRef.current = localTail;
  const localLoadingRef = useRef(localLoading);
  localLoadingRef.current = localLoading;
  /** Last record of the previous poll's window — how the next poll measures
   *  what the agent appended. Cleared with the messages it describes. */
  const prevLastIdRef = useRef<string | null>(null);

  // Optimistic follow-ups: submitting a resume/enqueue spawns a detached
  // `claude --resume` that only writes the message into the JSONL once the CLI
  // has cold-started (seconds). We echo the user's text immediately so the
  // conversation isn't blank during that window, then drop it once the real
  // transcript row lands. `resumeGrace` arms the tail pollers right away rather
  // than waiting for rescan to flip the session to `live`.
  const [optimisticSends, setOptimisticSends] = useState<OptimisticSend[]>([]);
  const [resumeGrace, setResumeGrace] = useState(false);
  const optimisticSeq = useRef(0);

  // External sessionInfo prop changed → reset local state and refetch.
  useEffect(() => {
    if (!isStandalone) return;
    if (sessionInfo && sessionInfo.id !== localSession?.id) {
      setLocalSession(sessionInfo);
      setLocalMessages([]);
      prevLastIdRef.current = null;
      setLocalLoadingEarlier(false);
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
    setLocalStalled(false);
    // dsh:// fetches log their whole lifecycle: the Rust side of this command
    // logs entry/exit for the same paths, so if a request goes dark the log
    // says on which side of the IPC boundary it happened. Claude paths stay
    // quiet — they poll every 1.5s and would flood the log.
    const probeDsh = localSession.jsonlPath?.startsWith("dsh://")
      ? (event: string) => {
          void invoke("log_frontend_debug", {
            msg: `tail-fetch ${event} tail=${tail} [${localSession.jsonlPath}]`,
          }).catch(() => {});
        }
      : null;
    probeDsh?.("fired");
    // Deadline, not abort: `get_messages_tail` can stay pending forever when the
    // backend stops answering (proven by freezing dsh's web server — the pane
    // sat on 「加载中…」 for 80s+ with no error). A late result still renders.
    withStallWatch(
      invoke<RawMessage[]>("get_messages_tail", {
        jsonlPath: localSession.jsonlPath,
        tail,
      }),
      () => {
        probeDsh?.(cancelled ? "stalled(cancelled)" : "stalled");
        if (cancelled) return;
        setLocalLoading(false);
        setLocalStalled(true);
      },
    )
      .then((msgs) => {
        probeDsh?.(`resolved ${msgs.length} msgs${cancelled ? " (cancelled)" : ""}`);
        if (cancelled) return;
        setLocalMessages((prev) => reconcileMessages(prev, msgs));
        setLocalFullyLoaded(msgs.length < tail);
        setLocalLoading(false);
        setLocalStalled(false);
      })
      .catch((e) => {
        probeDsh?.(`rejected ${String(e).slice(0, 200)}${cancelled ? " (cancelled)" : ""}`);
        if (cancelled) return;
        setLocalLoading(false);
        // A rejection with nothing on screen used to render as a silent blank
        // pane; say so instead, and give the reader a retry.
        setLocalStalled(true);
      });
    return () => {
      cancelled = true;
    };
  }, [isStandalone, localSession?.jsonlPath, paused, reloadKey]);

  const standaloneLoadEarlier = useCallback(async () => {
    if (!isStandalone || !localSession || localFullyLoaded) return;
    const nextTail = localTail + LOAD_EARLIER_STEP;
    setLocalLoading(true);
    setLocalLoadingEarlier(true);
    setLocalTail(nextTail);
    try {
      const msgs = await invoke<RawMessage[]>("get_messages_tail", {
        jsonlPath: localSession.jsonlPath,
        tail: nextTail,
      });
      setLocalMessages((prev) => reconcileMessages(prev, msgs));
      setLocalFullyLoaded(msgs.length < nextTail);
    } finally {
      setLocalLoading(false);
      setLocalLoadingEarlier(false);
    }
  }, [isStandalone, localSession?.jsonlPath, localTail, localFullyLoaded]);

  // Resolved values — flip between local mirror and global store based on mode.
  const session = isStandalone ? localSession : global.session;
  const messages = isStandalone ? localMessages : global.messages;
  const isLoading = isStandalone ? localLoading : global.isLoading;
  const loadStalled = isStandalone ? localStalled : global.loadStalled;
  const retryLoad = useCallback(() => {
    if (isStandalone) setReloadKey((k) => k + 1);
    else void global.retryLoad();
  }, [isStandalone, global.retryLoad]);
  const fullyLoaded = isStandalone ? localFullyLoaded : global.fullyLoaded;
  const searchQuery = isStandalone ? standaloneSearchQuery : global.searchQuery;
  const close = global.close;
  const open = useCallback(
    (s: SessionInfo) => {
      if (isStandalone) {
        setLocalSession(s);
        setLocalMessages([]);
        prevLastIdRef.current = null;
        setLocalTail(INITIAL_TAIL);
        setLocalFullyLoaded(false);
      } else {
        global.open(s);
      }
    },
    [isStandalone, global.open],
  );
  const loadEarlier = isStandalone ? standaloneLoadEarlier : global.loadEarlier;
  const isLoadingEarlier = isStandalone
    ? localLoadingEarlier
    : isLoading && messages.length > 0;
  const syncingLatest = showLatestSync({
    isLoading,
    isLoadingEarlier,
    messageCount: messages.length,
  });

  // Text of every real user row already in the transcript, so we can tell which
  // optimistic sends have landed and drop them (dedup by trimmed text).
  const realUserTexts = useMemo(() => {
    const set = new Set<string>();
    for (const m of messages) {
      if (m.type === "user") set.add(messageToText(m).trim());
    }
    return set;
  }, [messages]);

  // Optimistic sends that haven't yet appeared in the real transcript.
  const pendingOptimistic = useMemo(
    () => optimisticSends.filter((o) => !realUserTexts.has(o.text.trim())),
    [optimisticSends, realUserTexts],
  );

  // Once a send has landed in the real transcript, prune it from state so the
  // list doesn't keep re-appending it (and the memo above stays cheap). Only
  // set state when something actually changed, to avoid a render loop.
  useEffect(() => {
    setOptimisticSends((prev) => {
      const next = prev.filter((o) => !realUserTexts.has(o.text.trim()));
      return next.length === prev.length ? prev : next;
    });
  }, [realUserTexts]);

  // Let the grace window expire so the pollers fall back to status-driven
  // arming once the resumed turn is well underway (or has already gone live).
  useEffect(() => {
    if (!resumeGrace) return;
    const timer = window.setTimeout(() => setResumeGrace(false), RESUME_GRACE_MS);
    return () => window.clearTimeout(timer);
  }, [resumeGrace]);

  // Called by ResumeComposer the moment a follow-up is accepted by the backend.
  // Only a *resume* is being delivered now, so only it earns a transcript bubble
  // + poller grace; an *enqueue* is merely queued (turn still running, already
  // live), and its honest affordance is the "已排队" pending chip.
  const handleResumed = useCallback((finalPrompt: string, mode: "resume" | "enqueue") => {
    if (mode !== "resume") return;
    const text = finalPrompt.trim();
    if (text) {
      optimisticSeq.current += 1;
      const id = `optimistic-${Date.now()}-${optimisticSeq.current}`;
      setOptimisticSends((prev) => [...prev, { id, text }]);
    }
    setResumeGrace(true);
  }, []);

  const sessions = useSessionsStore((s) => s.sessions);
  const connection = useConnectionStore((s) => s.connection);
  const liveSession = useMemo(() => {
    if (!session) return null;
    return sessions.find((s) => s.id === session.id) ?? session;
  }, [session, sessions]);
  const preferredTitle = liveSession ? preferredSessionTitle(liveSession) : null;
  const pendingDecisions = useDecisionStore((s) => s.decisions);
  const [decisionRecords, setDecisionRecords] = useState<DecisionHistoryRecord[]>([]);
  const timelineMessages = useMemo(
    () => withCodexDecisionHistory(liveSession, messages, decisionRecords),
    [liveSession, messages, decisionRecords],
  );
  // Durable transcript/history rows first; optimistic user bubbles always stay
  // at the live edge and disappear once their real transcript row lands.
  const displayedMessages = useMemo(
    () => pendingOptimistic.length === 0
      ? timelineMessages
      : [...timelineMessages, ...pendingOptimistic.map(optimisticToMessage)],
    [timelineMessages, pendingOptimistic],
  );
  const inlineFleetAsk = useMemo(
    () => inlineCodexFleetAsk(liveSession, pendingDecisions, decisionRecords),
    [liveSession, pendingDecisions, decisionRecords],
  );
  const reasoningPercent =
    liveSession && liveSession.totalOutputTokens > 0
      ? (liveSession.reasoningOutputTokens / liveSession.totalOutputTokens) * 100
      : 0;

  // Build tabs: [mainSession, ...activeSubagents]
  // Show tabs only when viewing a main agent that has active subagents,
  // or when viewing a subagent (show sibling tabs + parent).
  const scrollRef = useRef<HTMLDivElement>(null);
  const [isFollowing, setIsFollowing] = useState(true);
  // The composer floats over the bottom of the transcript (chatbot-style), so
  // the scroller has to reserve exactly its height as bottom padding or the
  // last message hides underneath it. Measured, not guessed: the box grows with
  // the draft, the option pills and the queued-follow-up chips.
  const dockRef = useRef<HTMLDivElement>(null);
  const [dockHeight, setDockHeight] = useState(0);
  /* The conversation is no longer one tab among many — it owns this column for
     good. Everything that used to sit beside it in that row (Skills, 决策,
     Token, 任务, 后台任务, 临时文件, Workflow) is now a *button* that pulls the
     panel up in the auxiliary column, so reading a token receipt no longer
     costs you sight of the transcript. See detailAux.ts for the state. */
  const [aux, setAux] = useState<AuxState>(initialAux);
  /** Panel width, only meaningful in the side-by-side (non-overlay) form. */
  const {
    width: auxWidth,
    isDragging: auxDragging,
    onMouseDown: onAuxResize,
  } = useResizableWidth("detail-aux-width", { min: 260, max: 720, initial: 380, side: "right" });
  /** Pane width, measured — the aux column collapses to an overlay drawer below
   *  AUX_OVERLAY_PX. Measured rather than a media query because the constraint
   *  is this pane's width (one of four split groups, or DecisionPanel's inline
   *  column), not the window's. */
  const rootRef = useRef<HTMLDivElement>(null);
  const [paneWidth, setPaneWidth] = useState(0);
  /* The header's numeric chips — spend, tokens, reasoning share, compactions —
     are reference figures you look up, not identity you read at a glance. Seven
     of them in a row turned the title area into a status bar, so they collapse
     behind one toggle and the row keeps only what names the session. Context %
     is the exception: see the render for why the warn state stays out. */
  const [metricsOpen, setMetricsOpen] = useState(false);
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
  // `resumeGrace` keeps the tail/live-thinking pollers armed in the seconds
  // right after a submit, before rescan flips the session to a live status.
  const liveActive = (!!liveSession && shouldFollowSession(liveSession)) || resumeGrace;
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
          setLocalMessages((prev) => reconcileMessages(prev, msgs));
          setLocalFullyLoaded(msgs.length < tail);
          // Keep the window's start pinned as the transcript grows, so nothing
          // the reader has scrolled back to slides out of the top. Growth is
          // measured against the previous window's last record — see
          // `liveTailWindow` for the rule and what it costs to get wrong.
          const arrived = arrivedSince(prevLastIdRef.current, msgs);
          prevLastIdRef.current = recordId(msgs[msgs.length - 1]);
          const grown = nextLiveTail({ tail, returned: msgs.length, arrived });
          if (grown !== tail) setLocalTail(grown);
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
    // Switching sessions must not carry another session's pending echo over.
    setOptimisticSends([]);
    setResumeGrace(false);
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

    const refresh = () =>
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

    void refresh();
    const unsubscribe = subscribeDecisionHistoryRefresh(refresh);
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [liveSession?.id, liveSession?.jsonlPath]);

  // Honor an explicit initial facet (e.g. the user clicked the card's plan row
  // → open straight to 任务). Opens it in the aux column; the conversation is
  // always on screen either way.
  useEffect(() => {
    if (isStandalone) return;
    const facet = global.initialTab;
    if (!facet || !isAuxFacet(facet)) return;
    setAux((st) => showTab(st, facet));
  }, [isStandalone, global.session?.id, global.initialTab]);

  // TASKS.md plan for THIS session — scoped to the plan the session is focused
  // on (via its task-progress record), not every plan the workspace ever had.
  // Fetched through the Backend trait so it works for both local and remote
  // sessions. No session id → nothing to scope to → show nothing.
  const workspacePath = liveSession?.workspacePath;
  const sessionId = liveSession?.id;

  /** Open a doc — a repo file, a wiki doc or a url the agent named — in the
   *  auxiliary column. Every surface that renders agent prose routes here:
   *  the thing the transcript named opens beside the sentence that named it,
   *  instead of taking over the window (the 仓库 / 知识库 pages) or landing in
   *  the window's tab strip, where reading it cost sight of the conversation. */
  const openAuxDoc = useCallback((kind: AuxDocKind, ref: string) => {
    setAux((st) => openDoc(st, kind, ref));
  }, []);

  // Paths the agent wrote in backticks become clickable chips. Memoised because
  // MessageRow is memo'd — a fresh object each render would re-render every row.
  const pathLinks = useMemo<PathLinkContext | undefined>(() => {
    if (!workspacePath) return undefined;
    return {
      workspaceRoot: workspacePath,
      isLocal: connection?.type !== "remote",
      openInFiles: (absPath) => openAuxDoc("file", absPath),
    };
  }, [workspacePath, connection?.type, openAuxDoc]);

  // `[[slug]]` refs the agent wrote become links. Agents are told to publish
  // findings to the wiki and to cross-reference them that way, so the refs were
  // already all over the transcripts — as plain text, because nothing here had
  // ever handed the renderer a wiki context. Provided (not threaded) for the
  // reason spelled out in wikiLinksContext.
  //
  // Unknown slugs render grayed out rather than clickable, which is exactly the
  // signal worth having: it marks a doc the agent said it would write and
  // didn't.
  const { docs: wikiDocs } = useWikiDocs();
  const wikiLinks = useMemo<WikiLinkContext>(() => {
    const slugs = new Set(wikiDocs.map((d) => d.slug));
    return {
      hasSlug: (slug) => slugs.has(slug),
      // Beside the prose, same as a clicked path.
      openSlug: (slug) => openAuxDoc("wiki", slug),
    };
  }, [wikiDocs, openAuxDoc]);

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

  const pickTab = useCallback((id: string) => {
    setAux((st) => toggleTab(st, id));
  }, []);
  const closeAuxPanel = useCallback(() => {
    setAux((st) => closeAux(st));
  }, []);
  const openWebInAux = useCallback(
    (url: string) => {
      openAuxDoc("web", url);
    },
    [openAuxDoc],
  );
  const dropDoc = useCallback((id: string) => {
    setAux((st) => closeDoc(st, id));
  }, []);

  // Pane width drives the overlay/side-by-side choice.
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const measure = () => setPaneWidth(el.clientWidth);
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [liveSession?.id]);

  // `isFollowing` drives the footer, but the pin below runs from a
  // ResizeObserver callback that must not re-subscribe on every state change —
  // so mirror the state into a ref and write both through one reducer.
  const followRef = useRef<FollowState>(initialFollowState);
  const applyFollow = useCallback((input: FollowInput) => {
    const next = nextFollowState(followRef.current, input);
    if (next.following === followRef.current.following && next.detached === followRef.current.detached) {
      return;
    }
    followRef.current = next;
    setIsFollowing(next.following);
  }, []);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      applyFollow({
        kind: "scroll",
        distFromBottom: el.scrollHeight - el.scrollTop - el.clientHeight,
      });
    };
    // An upward gesture detaches immediately, before the scroll it causes is
    // even dispatched. Position alone can't express "I want to read back": the
    // reader is still inside the slack window at that point, so the distance
    // rule would keep following and the pin would drag them back down.
    const onWheel = (ev: WheelEvent) => {
      // Wheel bubbles, so scrolling *inside* a card in the transcript lands
      // here too. Reading back through a subagent's result is not a request to
      // stop following the transcript, so let the card have its own gesture.
      if (nestedScrollerWillConsume(el, ev)) return;
      applyFollow({ kind: "gesture", intent: ev.deltaY });
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    el.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      el.removeEventListener("wheel", onWheel);
    };
  }, [applyFollow, session]);

  // A different session starts pinned again.
  useEffect(() => {
    followRef.current = initialFollowState;
    setIsFollowing(true);
  }, [liveSession?.id]);

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
    const el = scrollRef.current;
    if (!el) return;

    const pin = () => {
      if (!followRef.current.following) return;
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
  }, [liveSession?.id, hasMessages, hasLiveThinking, inlineFleetAsk?.id]);

  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, []);

  // ── Scroll-freeze snapshot (⌥⇧S) ────────────────────────────────────────────
  // The transcript occasionally refuses to scroll until the window is resized,
  // and no explanation has survived scrutiny yet — see `scrollSnapshot.ts` for
  // what the readings are meant to separate. Take one while it is stuck and one
  // after the resize that released it; both land on the clipboard.
  //
  // A hotkey rather than a devtools one-liner because opening the inspector
  // resizes the webview, which is the very thing known to clear the freeze: the
  // act of going to look would destroy the state being looked at.
  const [snapshots, setSnapshots] = useState<string[]>([]);
  // Render-synced so the keydown listener (mounted once) reads current values
  // without re-subscribing. What the DOM cannot say about itself: a pane
  // measured holding one message while its owner had 1621 renderable records is
  // only a contradiction once both halves are in the same reading.
  const probeCountsRef = useRef<Record<string, string | number | boolean>>({});
  probeCountsRef.current = {
    msgs: messages.length,
    displayed: displayedMessages.length,
    renderable: displayedMessages.filter(isRenderableRow).length,
    tail: isStandalone ? localTail : global.loadedTail ?? -1,
    fullyLoaded,
    isLoading,
    stalled: loadStalled,
    following: followRef.current.following,
    detached: followRef.current.detached,
    // The conversation is no longer a tab; what varies is which auxiliary
    // panel is up beside it (null = closed).
    tab: aux.active ?? "—",
    dockH: dockHeight,
  };
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (!ev.altKey || !ev.shiftKey || ev.code !== "KeyS") return;
      const el = scrollRef.current;
      if (!el) return;
      ev.preventDefault();
      const stamp = new Date().toTimeString().slice(0, 8);
      const text = formatSnapshot(
        takeScrollSnapshot(el, currentViewMetrics(), stamp, probeCountsRef.current),
      );
      setSnapshots((prev) => [...prev, text]);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  useEffect(() => {
    if (snapshots.length === 0) return;
    writeText(snapshots.join("\n\n")).catch(() => {});
  }, [snapshots]);

  // Keep the transcript's bottom padding equal to the floating dock's height.
  // The dock mounts and unmounts with the composer / follow pill, so this
  // re-subscribes on those flags; its *size* changes (a growing draft) come
  // through the observer.
  const showsComposer = (canResume || canEnqueue) && !!liveSession;
  useEffect(() => {
    const el = dockRef.current;
    if (!el) {
      setDockHeight(0);
      return;
    }
    const measure = () => setDockHeight(el.offsetHeight);
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [showsComposer, isFollowing, liveSession?.id]);

  // Padding is applied by React on the next paint, which grows scrollHeight
  // under a reader who is pinned to the newest message. Re-pin in the same
  // frame so a composer that expands while typing doesn't leave the last
  // message drifting up behind it.
  useLayoutEffect(() => {
    if (!followRef.current.following) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [dockHeight]);

  /** Subagents of this session family that are running *right now* — the deck
   *  of cards pinned to the top of the auxiliary column. Workflow fan-out
   *  agents are included (unlike the scope dropdown, which excludes them to
   *  stay a menu): "看完整个任务的所有 agent 状态" means all of them, and the
   *  deck caps its render rather than its input. Sorted most-recently-active
   *  first so the cap keeps the ones actually moving. */
  const liveSubagents = useMemo((): SessionInfo[] => {
    if (!liveSession) return [];
    const parentId = liveSession.isSubagent
      ? liveSession.parentSessionId
      : liveSession.id;
    if (!parentId) return [];
    return sessions
      .filter(
        (s) =>
          s.isSubagent &&
          s.parentSessionId === parentId &&
          s.id !== liveSession.id &&
          LIVE_STATUSES.has(s.status),
      )
      .sort((a, b) => b.lastActivityMs - a.lastActivityMs);
  }, [liveSession, sessions]);

  // A dismissal of the agent deck is spent once the last one finishes, so the
  // next fan-out earns a fresh auto-open.
  useEffect(() => {
    setAux((st) => syncLiveAgents(st, liveSubagents.length));
  }, [liveSubagents.length]);

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
    // goes Idle (or stalls at RateLimited) it used to vanish from the selector,
    // leaving the only way back into its transcript the "open subagent" button
    // buried in its Agent card. Order active first, then most-recently-active
    // finished ones, and cap the list — a parent that fanned out dozens/hundreds
    // of subagents would otherwise flood the scope dropdown. The menu scrolls
    // (see AgentScopeSwitcher) so the capped set stays reachable.
    const active = subagents.filter((s) => LIVE_STATUSES.has(s.status));
    const finished = subagents
      .filter((s) => !LIVE_STATUSES.has(s.status))
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

  // The auxiliary column's tab strip: the running agents, the session's facets,
  // then every doc opened from the transcript. Conditional facets appear on the
  // same terms their old tabs did — only when the session has something to show.
  const auxTabs = useMemo((): AuxTab[] => {
    const list: AuxTab[] = [];
    if (liveSubagents.length > 0) {
      list.push({
        id: AGENTS_TAB,
        label: t("detail.live_agents", { count: liveSubagents.length }),
      });
    }
    list.push({ id: "skills", label: t("detail.tab_skills") });
    list.push({ id: "decisions", label: t("detail.tab_decisions") });
    list.push({ id: "tokens", label: t("detail.tab_tokens") });
    if (hasTaskPlans) list.push({ id: "tasks", label: t("detail.tab_tasks") });
    if (hasBgTasks) {
      list.push({ id: "bgtasks", label: `${t("detail.tab_bgtasks")} (${bgTasks.length})` });
    }
    if (hasScratchpad) {
      list.push({
        id: "scratchpad",
        label: `${t("detail.tab_scratchpad")} (${scratchpadCount})`,
      });
    }
    if (hasWorkflows) {
      list.push({
        id: "workflow",
        label: `${t("detail.tab_workflow")} (${workflowTrees.length})`,
      });
    }
    for (const d of aux.docs) list.push({ id: d.id, label: d.label, closable: true });
    return list;
  }, [
    t,
    liveSubagents.length,
    hasTaskPlans,
    hasBgTasks,
    bgTasks.length,
    hasScratchpad,
    scratchpadCount,
    hasWorkflows,
    workflowTrees.length,
    aux.docs,
  ]);

  // A selection whose tab has since disappeared (the session took another turn
  // and emptied 后台任务, say) would otherwise hold the panel on nothing.
  useEffect(() => {
    const ids = new Set(auxTabs.map((tb) => tb.id));
    setAux((st) => pruneTab(st, (id) => ids.has(id)));
  }, [auxTabs]);

  const activeTab = activeAuxTab(aux, liveSubagents.length);
  const auxOpen = activeTab != null;
  const activeFacet = activeTab != null && isAuxFacet(activeTab) ? activeTab : null;
  const activeDoc = activeTab == null ? null : aux.docs.find((d) => d.id === activeTab) ?? null;
  // Overlay until the pane is wide enough for two columns. `paneWidth === 0` is
  // the pre-measure frame; treat it as wide so the panel doesn't flash as an
  // overlay on mount.
  const auxOverlay = paneWidth > 0 && paneWidth < AUX_OVERLAY_PX;
  // The toolbar switch: hide it when it is showing, and bring back the tab the
  // reader was last on (the agent deck, if agents are running) when it is not.
  const reopenTabId = auxTabs[0]?.id ?? "skills";
  const toggleAuxPanel = useCallback(() => {
    setAux((st) =>
      activeAuxTab(st, liveSubagents.length) == null ? showTab(st, reopenTabId) : closeAux(st),
    );
  }, [liveSubagents.length, reopenTabId]);

  return (
    // Both link capabilities cover the whole component, so the reader modal and
    // every tool-block renderer inherit them too — and both now land in the
    // auxiliary column, which every instance of this component has.
    <WikiLinksProvider value={wikiLinks}>
      <WebLinkProvider value={openWebInAux}>
      <div
        ref={rootRef}
        className={`${styles.root} ${liveSession ? styles.open : ""} ${inline ? styles.inline : ""} ${auxOpen ? styles.aux_open : ""}`}
      >
        {liveSession && (
          <>
          {/* The gutter around the two slabs is now what reaches the window's
              top edge, so it carries its own drag region — same reason the hero
              and the aux tab strip do (Tauri's shim reads e.target, not an
              ancestor). The resize handle inside it is a child without the
              attribute, so col-resize dragging still wins there. */}
          <div className={styles.body_row} data-tauri-drag-region>
            <div className={styles.main_col}>
              {/* Hero banner. The session's identity and the controls that act
                  on it, as one surface rather than a title row with a tab strip
                  bolted under it — there are no tabs on this side any more, so
                  nothing here should look like one. The AI title leads (it is
                  what identifies the session); everything you only ever copy
                  (session id, transcript path, workspace path) lives behind the
                  ⋯ menu; the plan / handoff / watch rows ride along the bottom
                  edge, where they stay put instead of scrolling away with the
                  conversation. */}
              {/* data-tauri-drag-region on every container of this banner: it
                  now owns the window's top-right corner, and a frameless window
                  can only be dragged by an element that carries the attribute
                  itself (Tauri's shim reads e.target, not an ancestor). The
                  fixed <WindowsFrameOverlay> strip above is pointer-events:none,
                  so it does not cover this corner on macOS — every other surface
                  reaching the window top (sidebar header, PageShell banner)
                  carries its own region for the same reason. Buttons and chips
                  are separate targets, so their clicks are unaffected. */}
              <div className={styles.hero} data-tauri-drag-region>
                <div className={styles.hero_top} data-tauri-drag-region>
                  <div className={styles.hero_ident} data-tauri-drag-region>
                    <div
                      className={styles.header_title}
                      title={preferredTitle || liveSession.workspacePath}
                      data-tauri-drag-region
                    >
                      {preferredTitle || liveSession.workspaceName}
                    </div>
                  <div className={styles.meta_row} data-tauri-drag-region>
                    {/* Only when the title line isn't already the workspace name. */}
                    {preferredTitle && preferredTitle !== liveSession.workspaceName && (
                      <span
                        className={styles.workspace_chip}
                        title={liveSession.workspacePath}
                      >
                        {liveSession.workspaceName}
                      </span>
                    )}
                    {/* Agent scope: which member of the session family every facet is
                        scoped to. A dropdown (not the old in-row segmented strip) so a
                        growing subagent list never crowds the view tabs. */}
                    <AgentScopeSwitcher tabs={tabs} current={liveSession} onOpen={open} />
                    {liveSession.model && (
                      <span
                        className={styles.meta_chip}
                        title={t("card.tip_model", { model: liveSession.model })}
                      >
                        {formatModel(liveSession.model)}
                      </span>
                    )}
                    {/* Reasoning effort — same lightbulb the cards use, but WITHOUT
                        their `medium` cut. On a dense board a chip on every card says
                        nothing; the detail header is the one place you come to ask
                        what this session is actually running at, so `medium` is an
                        answer there. */}
                    {effortChipLabel(liveSession) && (
                      <span
                        className={styles.meta_chip}
                        title={effortTitle(t, liveSession)}
                      >
                        <svg viewBox="0 0 8 11" width="9" height="9" fill="currentColor" aria-hidden>
                          <path d="M4 0.5 C1.2 0.5 0.5 2.8 0.5 4.5 C0.5 6.3 1.8 7.4 2.3 8 L2.3 9.3 L5.7 9.3 L5.7 8 C6.2 7.4 7.5 6.3 7.5 4.5 C7.5 2.8 6.8 0.5 4 0.5Z" />
                        </svg>
                        {effortChipLabel(liveSession)}
                      </span>
                    )}
                    {/* Context stays out of the fold once it crosses the warn line.
                        Below it, it is a figure; above it, it is an alarm — the
                        session is about to compact — and an alarm you have to click
                        to see is not an alarm. */}
                    {liveSession.contextPercent != null &&
                      (metricsOpen || liveSession.contextPercent >= 0.8) && (
                      <span
                        className={`${styles.meta_chip} ${liveSession.contextPercent >= 0.8 ? styles.meta_chip_warn : ""}`}
                        title={t("card.tip_context", { percent: Math.round(liveSession.contextPercent * 100) })}
                      >
                        ctx {Math.round(liveSession.contextPercent * 100)}%
                      </span>
                    )}
                    {metricsOpen && (liveSession.totalCostUsd ?? 0) >= 0.005 && (
                      <span className={styles.meta_chip} title={t("card.tip_cost")}>
                        ${liveSession.totalCostUsd.toFixed(2)}
                      </span>
                    )}
                    {metricsOpen && (
                      <span className={styles.meta_chip} title={t("tokens_out")}>
                        {liveSession.totalOutputTokens.toLocaleString()} tok
                      </span>
                    )}
                    {metricsOpen && liveSession.reasoningOutputTokens > 0 && (
                      <span
                        className={styles.meta_chip}
                        title={t("reasoning_tokens_tip", {
                          tokens: liveSession.reasoningOutputTokens.toLocaleString(),
                          percent: reasoningPercent.toFixed(1),
                        })}
                      >
                        {t("reasoning_tokens_chip", {
                          tokens: liveSession.reasoningOutputTokens.toLocaleString(),
                          percent: reasoningPercent.toFixed(1),
                        })}
                      </span>
                    )}
                    {metricsOpen && (liveSession.compactCount ?? 0) > 0 && (
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
                    <ScheduleProvenanceChip session={liveSession} />
                    {/* Reveals the numeric chips above. Sits last so the identity run
                        reads uninterrupted and the control lands at the row's end. */}
                    <button
                      type="button"
                      className={`${styles.metrics_toggle} ${metricsOpen ? styles.metrics_toggle_open : ""}`}
                      onClick={() => setMetricsOpen((v) => !v)}
                      title={t("detail.metrics") || "Session metrics"}
                      aria-expanded={metricsOpen}
                    >
                      {metricsOpen ? "×" : "···"}
                    </button>
                  </div>
                  </div>
                  {/* Toolbar. The auxiliary column's switch leads it: with the
                      facet buttons gone from this side, this is how you get the
                      panel back once it is closed. */}
                  <div className={styles.hero_tools} data-tauri-drag-region>
                    <button
                      type="button"
                      className={`${styles.hero_tool} ${auxOpen ? styles.hero_tool_on : ""}`}
                      onClick={toggleAuxPanel}
                      aria-pressed={auxOpen}
                      title={auxOpen ? t("detail.aux_hide", "收起辅助栏") : t("detail.aux_show", "展开辅助栏")}
                      aria-label={auxOpen ? t("detail.aux_hide", "收起辅助栏") : t("detail.aux_show", "展开辅助栏")}
                    >
                      <PanelRight size={14} strokeWidth={1.8} />
                      {liveSubagents.length > 0 && (
                        <span className={styles.hero_tool_badge}>{liveSubagents.length}</span>
                      )}
                    </button>
                    <SessionHeaderMenu
                      sessionId={liveSession.id}
                      jsonlPath={liveSession.jsonlPath}
                      workspacePath={liveSession.workspacePath}
                      isLocal={connection?.type !== "remote"}
                    />
                    {!inline && (
                      <button
                        className={styles.close_btn}
                        onClick={close}
                        title={t("common.close") || "Close"}
                      >
                        ✕
                      </button>
                    )}
                  </div>
                </div>
                {/* Pinned plan.
                    Cursor keeps plans as first-class objects in its sidebar, Jules
                    gives the plan its own card above the activity feed, Devin has a
                    Progress tab — the shape they converge on is that the plan does
                    not scroll away with the work. Fleet already had this row on the
                    session cards and the data on SessionInfo; it was only missing
                    where you actually read the run. Clicking opens the Tasks tab,
                    the same destination as from a card. */}
                {liveSession.taskPlan && (
                  <PlanProgressRow
                    plan={liveSession.taskPlan}
                    variant="header"
                    onOpen={() => setAux((st) => showTab(st, "tasks"))}
                  />
                )}
                {/* Handoff relay chain — chip toggles the chain detail panel */}
                {liveSession.handoff && <HandoffChainRow session={liveSession} />}
                {/* Active fleet-watch(es) — what this session is waiting on */}
                {liveSession.watches && liveSession.watches.length > 0 && (
                  <WatchStatusRow session={liveSession} />
                )}
              </div>

              <div className={styles.messages_pane}>
                {syncingLatest && (
                  <div className={styles.syncing_latest} role="status" aria-live="polite">
                    <LoaderCircle size={14} aria-hidden="true" />
                    {t("detail.syncing_latest", "正在同步最新消息…")}
                  </div>
                )}
                <div
                  ref={scrollRef}
                  className={styles.scroll_area}
                  style={{ paddingBottom: dockHeight }}
                >
                  {/* The "load earlier" control lives inside MessageList, which
                      owns the render window this button used to duplicate. */}
                  <AgentNavProvider nav={agentNav}>
                    <MessageList
                      messages={displayedMessages}
                      isLoading={isLoading}
                      stalled={loadStalled}
                      onRetry={retryLoad}
                      searchQuery={searchQuery}
                      status={liveSession?.status ?? null}
                      liveThinking={liveThinking}
                      decisionRecords={decisionRecords}
                      onLoadEarlier={loadEarlier}
                      fullyLoaded={fullyLoaded}
                      isLoadingEarlier={isLoadingEarlier}
                      paths={pathLinks}
                      // Use the live-refreshed session (same source every other
                      // jsonlPath consumer here uses); `session` is the possibly-
                      // stale object the drawer was opened with, whose jsonlPath
                      // can be absent for sessions opened from a partial shape.
                      jsonlPath={liveSession?.jsonlPath ?? session?.jsonlPath}
                    />
                    {inlineFleetAsk && (
                      <div className={styles.inline_fleet_ask} data-testid="inline-codex-fleet-ask">
                        <Suspense fallback={<div className={styles.inline_fleet_ask_loading}>…</div>}>
                          <InlineFleetAskCard decision={inlineFleetAsk} compact />
                        </Suspense>
                      </div>
                    )}
                  </AgentNavProvider>
                </div>

                {/* Composer + follow control, floating over the bottom of the
                    transcript instead of sitting in a separate docked bar below
                    it: same reading column as the messages, and the conversation
                    scrolls under it behind a fade. The scroller reserves this
                    element's measured height as bottom padding.

                    Resume/enqueue: when the turn has ended submit resumes; while
                    the turn is still running it queues a follow-up (delivered
                    when the turn ends). One of canResume / canEnqueue holds. */}
                {(showsComposer || !isFollowing) && (
                  <div ref={dockRef} className={styles.dock_layer}>
                    {!isFollowing && (
                      <button className={styles.follow_pill} onClick={scrollToBottom}>
                        ↓ {t("detail.scroll_to_latest")}
                      </button>
                    )}
                    {showsComposer && liveSession && (
                      <div className={styles.resume_dock}>
                        <ResumeComposer
                          sessionId={liveSession.id}
                          workspacePath={liveSession.workspacePath}
                          agentSource={liveSession.agentSource}
                          session={liveSession}
                          onResumed={handleResumed}
                          mode={canEnqueue ? "enqueue" : "resume"}
                          pendingMessages={liveSession.pendingMessages ?? []}
                        />
                      </div>
                    )}
                  </div>
                )}
              </div>
            </div>

            {auxOpen && (
              <SessionAuxPanel
                overlay={auxOverlay}
                width={auxWidth}
                isDragging={auxDragging}
                onResizeStart={onAuxResize}
                tabs={auxTabs}
                activeId={activeTab}
                onPick={pickTab}
                onCloseTab={dropDoc}
                onClose={closeAuxPanel}
              >
                {activeTab === AGENTS_TAB && (
                  <SubagentLiveCards agents={liveSubagents} onOpen={open} />
                )}
                {activeDoc && (
                  <SessionAuxDoc
                    doc={activeDoc}
                    onOpenWiki={(slug) => openAuxDoc("wiki", slug)}
                    onClose={() => dropDoc(activeDoc.id)}
                  />
                )}
                {activeFacet && (
                  <SessionFacetPanel
                    facet={activeFacet}
                    session={liveSession}
                    decisionRecords={decisionRecords}
                    taskPlans={taskPlans}
                    bgTasks={bgTasks}
                    workflowTrees={workflowTrees}
                    sessions={sessions}
                    onOpenAgent={openAgentSession}
                  />
                )}
              </SessionAuxPanel>
            )}
          </div>
        </>
      )}
      {/* Portalled to <body> on purpose: a fixed overlay inside the pane would
          still be a layout mutation on the element under investigation. */}
      {snapshots.length > 0 &&
        createPortal(
          <div className={styles.snapshot_overlay}>
            <div className={styles.snapshot_head}>
              <span>scroll snapshots · {snapshots.length} · copied to clipboard</span>
              <button type="button" onClick={() => setSnapshots([])}>
                clear
              </button>
            </div>
            <pre className={styles.snapshot_body}>{snapshots.join("\n\n")}</pre>
          </div>,
          document.body,
        )}
      </div>
      </WebLinkProvider>
    </WikiLinksProvider>
  );
}
