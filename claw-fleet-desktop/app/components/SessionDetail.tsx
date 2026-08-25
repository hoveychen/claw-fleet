import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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
import { CalendarClock } from "lucide-react";
import { canResumeSession, canEnqueueSession, preferredSessionTitle, shouldFollowSession, LIVE_STATUSES, SCHEDULE_ENTRYPOINT } from "../types";
import type { DecisionHistoryRecord, LiveThinking, RawMessage, SessionInfo, TaskPlanDetail } from "../types";
import { messageToText } from "../messageRows";
import { reconcileMessages } from "../messageReuse";
import { withStallWatch } from "../loadDeadline";
import {
  initialFollowState,
  nextFollowState,
  type FollowInput,
  type FollowState,
} from "../followState";
import { installScrollFreezeProbe } from "../scrollFreezeProbe";
import { installScrollBoxResizeRevive } from "../scrollLayerRevive";
import { AgentNavProvider } from "./AgentNavContext";
import { DecisionHistory } from "./DecisionHistory";
import { HandoffChainRow } from "./HandoffChainRow";
import { PlanProgressRow } from "./PlanProgressRow";
import { WatchStatusRow } from "./WatchStatusRow";
import { MessageList } from "./MessageList";
import type { PathLinkContext } from "../markdown/pathLinks";
import type { WikiLinkContext } from "../markdown/wikiLinks";
import type { DetailTabOpener } from "../tabKind";
import { WikiLinksProvider } from "../markdown/wikiLinksContext";
import { WebLinkProvider } from "../markdown/webLinks";
import { revealSlugInWikiPage, useWikiDocs } from "../hooks/useWikiDocs";
import { ResumeComposer } from "./ResumeComposer";
import { ScratchpadView } from "./ScratchpadView";
import type { ExplorerEntry } from "./ExplorerPane";
import { SessionHeaderMenu } from "./SessionHeaderMenu";
import { AgentScopeSwitcher } from "./AgentScopeSwitcher";
import { formatModel } from "./SessionCard";
import { SkillHistory } from "./SkillHistory";
import { TokenSpendPanel } from "./TokenSpendPanel";
import { CodexTokenPanel } from "./CodexTokenPanel";
import { DshTokenPanel } from "./DshTokenPanel";
import { tokenPanelForAgentSource } from "../modelChoices";
import { inlineCodexFleetAsk } from "./codexDecision";
import { WorkflowDag } from "./blocks/WorkflowDag";
import { useWorkflowTrees } from "../hooks/useWorkflowTrees";
import { isWorkflowAgent } from "../workflowAgent";
import { bgTaskIcon, bgTaskDataType } from "../bgTaskKinds";
import { subscribeDecisionHistoryRefresh } from "../decisionHistoryRefresh";
import styles from "./SessionDetail.module.css";


/** Max subagents listed in the scope dropdown (AgentScopeSwitcher). Active ones
 *  win the slots first, then the most-recently-active finished ones. A parent
 *  that fanned out hundreds of subagents would otherwise flood the menu. */
const SUBAGENT_TAB_CAP = 12;

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
  lite = false,
  inline = false,
  sessionInfo = null,
  searchQuery: standaloneSearchQuery = null,
  paused = false,
  tabOpener,
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
  /** Present when this instance lives in a tab strip (the 任务 page's detail
   *  column): a path the agent named then opens as a tab *beside* the
   *  conversation instead of switching the whole window to the 仓库 page. Absent
   *  in the global drawer and Lite mode, which have nowhere to put a tab and so
   *  keep the page-switching behaviour. */
  tabOpener?: DetailTabOpener;
} = {}) {
  const { t } = useTranslation();
  const isStandalone = sessionInfo != null;
  const global = useDetailStore();

  // Local mirror state for standalone mode.
  const [localSession, setLocalSession] = useState<SessionInfo | null>(sessionInfo ?? null);
  const [localMessages, setLocalMessages] = useState<RawMessage[]>([]);
  const [localLoading, setLocalLoading] = useState(false);
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
        setLocalTail(INITIAL_TAIL);
        setLocalFullyLoaded(false);
      } else {
        global.open(s);
      }
    },
    [isStandalone, global.open],
  );
  const loadEarlier = isStandalone ? standaloneLoadEarlier : global.loadEarlier;

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

  // Real transcript + not-yet-landed optimistic bubbles, appended at the end.
  const displayedMessages = useMemo(
    () =>
      pendingOptimistic.length === 0
        ? messages
        : [...messages, ...pendingOptimistic.map(optimisticToMessage)],
    [messages, pendingOptimistic],
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
  const requestFileNav = useUIStore((s) => s.requestFileNav);
  const liveSession = useMemo(() => {
    if (!session) return null;
    return sessions.find((s) => s.id === session.id) ?? session;
  }, [session, sessions]);
  const preferredTitle = liveSession ? preferredSessionTitle(liveSession) : null;
  const pendingDecisions = useDecisionStore((s) => s.decisions);
  const inlineFleetAsk = useMemo(
    () => inlineCodexFleetAsk(liveSession, pendingDecisions),
    [liveSession, pendingDecisions],
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
  /* The header's numeric chips — spend, tokens, reasoning share, compactions —
     are reference figures you look up, not identity you read at a glance. Seven
     of them in a row turned the title area into a status bar, so they collapse
     behind one toggle and the row keeps only what names the session. Context %
     is the exception: see the render for why the warn state stays out. */
  const [metricsOpen, setMetricsOpen] = useState(false);
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
      // In a tab strip the file opens beside the prose that named it — the
      // whole point of an IDE's split. Elsewhere (drawer, Lite) there is no
      // strip, so it still goes to the 仓库 page.
      openInFiles: (absPath, line) =>
        tabOpener
          ? tabOpener.openFile(absPath, line)
          : requestFileNav({ workspacePath, absPath, line }),
    };
  }, [workspacePath, connection?.type, requestFileNav, tabOpener]);

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
      // A tab beside the prose where there is a strip; the 知识库 page otherwise
      // — the same split as a clicked path.
      openSlug: tabOpener ? tabOpener.openWiki : revealSlugInWikiPage,
    };
  }, [wikiDocs, tabOpener]);

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

  // Auto-follow writes that actually moved the viewport, read by the scroll
  // freeze probe to tell "the pin dragged it back" from "nothing moved at all".
  const pinCountRef = useRef(0);

  // Last content height change, also for the probe. `at: 0` means the content
  // has not changed since this transcript was opened — reported as
  // `sinceGrow=none`, which is a different fact from "grew 0px just now".
  const growthRef = useRef({ at: 0, byPx: 0, height: 0 });

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
      applyFollow({ kind: "gesture", intent: ev.deltaY });
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    el.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      el.removeEventListener("wheel", onWheel);
    };
  }, [applyFollow, session, viewTab]);

  // A different session, or a fresh visit to the tab, starts pinned again.
  useEffect(() => {
    followRef.current = initialFollowState;
    setIsFollowing(true);
  }, [liveSession?.id, viewTab]);

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

    // A fresh transcript has no growth history; the first observation below
    // establishes the baseline rather than counting as a change.
    growthRef.current = { at: 0, byPx: 0, height: el.scrollHeight };

    const pin = () => {
      if (!followRef.current.following) return;
      const before = el.scrollTop;
      el.scrollTop = el.scrollHeight;
      // Only count a pin that actually moved the viewport. Re-pinning a box
      // already at the bottom is a no-op, and counting those would let the
      // freeze probe blame the pin for gestures it never touched.
      if (el.scrollTop !== before) pinCountRef.current += 1;
    };

    // Timestamp content height changes for the freeze probe: they are what
    // WebKit bug 150974 blames for a scrollable container losing its scroll,
    // and this observer already fires on exactly that event, so the probe reads
    // it from here instead of installing a second one on the same children.
    const noteHeight = () => {
      const height = el.scrollHeight;
      if (height !== growthRef.current.height) {
        growthRef.current = {
          at: Date.now(),
          byPx: height - growthRef.current.height,
          height,
        };
      }
    };

    // Observe the children, not the scroll box: the box's own border box never
    // changes size, it is the content inside it that grows. Re-subscribing when
    // the child set changes (loading placeholder -> list, live thinking block)
    // is why those two flags are in the dependency list.
    const ro = new ResizeObserver(() => {
      noteHeight();
      pin();
    });
    for (const child of Array.from(el.children)) ro.observe(child);
    pin();
    return () => ro.disconnect();
  }, [viewTab, liveSession?.id, hasMessages, hasLiveThinking, inlineFleetAsk?.id]);

  // Render-synced mirror so the probe below reports the *current* session
  // without re-installing its listener on every poll.
  const probeStateRef = useRef({
    sessionId: null as string | null,
    status: null as string | null,
    messageCount: 0,
  });
  probeStateRef.current = {
    sessionId: liveSession?.id ?? null,
    status: liveSession?.status ?? null,
    messageCount: displayedMessages.length,
  };

  // Catch the conversation pane refusing to scroll. Reported repeatedly, never
  // caught in the act: it starts at no identifiable moment, a release build has
  // no devtools, and the state is gone by the time it gets described. The probe
  // watches every wheel gesture and writes one line to the host debug log when
  // a gesture that had somewhere to go doesn't land — with the metrics that say
  // whether it was a layout fault, the auto-follow pin, or the compositor.
  useEffect(() => {
    if (viewTab !== "messages") return;
    const el = scrollRef.current;
    if (!el) return;
    return installScrollFreezeProbe(
      el,
      {
        sessionId: () => probeStateRef.current.sessionId,
        status: () => probeStateRef.current.status,
        messageCount: () => probeStateRef.current.messageCount,
        isFollowing: () => followRef.current.following,
        pinCount: () => pinCountRef.current,
        contentGrowth: () => {
          const g = growthRef.current;
          return g.at === 0 ? null : { sinceMs: Date.now() - g.at, byPx: g.byPx };
        },
      },
      (line) => {
        // Absent outside Tauri (the mock browser preview); nothing to report to.
        invoke("log_frontend_debug", { msg: line }).catch(() => {});
      },
    );
  }, [viewTab, liveSession?.id]);

  // Rebuild the scroll layer whenever the box itself gets resized — the
  // squeeze from the composer dock swapping underneath is the transition the
  // freeze correlates with. Separate from the pin's ResizeObserver above,
  // which watches the *children*: this squeeze comes from a sibling, so only
  // this box's own border box changes.
  useEffect(() => {
    if (viewTab !== "messages") return;
    const el = scrollRef.current;
    if (!el) return;
    return installScrollBoxResizeRevive(el);
  }, [viewTab, liveSession?.id]);

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

  return (
    // Both link capabilities cover the whole component, so the reader modal and
    // every tool-block renderer inherit them too. `openWeb` is null outside a
    // tab strip, which is precisely "send it to the browser".
    <WikiLinksProvider value={wikiLinks}>
      <WebLinkProvider value={tabOpener?.openWeb ?? null}>
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
                title={preferredTitle || liveSession.workspacePath}
              >
                {preferredTitle || liveSession.workspaceName}
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
              {/* Reasoning effort — same field, lightbulb and tooltip the cards
                  use, and the same `medium` cut: that is Claude's default level
                  and a chip on every session says nothing. Also filtered here:
                  the bare `thinking` marker, which records that a transcript has
                  thinking blocks and names no level at all. */}
              {liveSession.thinkingLevel &&
                liveSession.thinkingLevel !== "medium" &&
                liveSession.thinkingLevel !== "thinking" && (
                <span
                  className={styles.meta_chip}
                  title={t("card.tip_thinking", { level: liveSession.thinkingLevel })}
                >
                  <svg viewBox="0 0 8 11" width="9" height="9" fill="currentColor" aria-hidden>
                    <path d="M4 0.5 C1.2 0.5 0.5 2.8 0.5 4.5 C0.5 6.3 1.8 7.4 2.3 8 L2.3 9.3 L5.7 9.3 L5.7 8 C6.2 7.4 7.5 6.3 7.5 4.5 C7.5 2.8 6.8 0.5 4 0.5Z" />
                  </svg>
                  {liveSession.thinkingLevel}
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
                onOpen={() => pickTab("tasks")}
              />
            )}
            {/* Handoff relay chain — chip toggles the chain detail panel */}
            {liveSession.handoff && <HandoffChainRow session={liveSession} />}
            {/* Active fleet-watch(es) — what this session is waiting on */}
            {liveSession.watches && liveSession.watches.length > 0 && (
              <WatchStatusRow session={liveSession} />
            )}
          </div>

          {/* View-tab row. The agent scope selector no longer lives here — it
              moved to the header as a dropdown (AgentScopeSwitcher) so a growing
              subagent list can't push these fixed tabs off the right edge. */}
          <div className={styles.tab_bar}>
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
            {
              // `jsonlPath` carries each source's own handle: a file path for
              // Claude, a `codex://` rollout URI, a `dsh://` session id.
              codex: <CodexTokenPanel jsonlPath={liveSession.jsonlPath} />,
              dsh: <DshTokenPanel uri={liveSession.jsonlPath} />,
              claude: (
                <TokenSpendPanel
                  jsonlPath={liveSession.jsonlPath}
                  workspacePath={liveSession.workspacePath}
                />
              ),
            }[tokenPanelForAgentSource(liveSession.agentSource)]
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
                    isLoadingEarlier={isLoading && messages.length > 0}
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
                    session={liveSession}
                    onResumed={handleResumed}
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
      </WebLinkProvider>
    </WikiLinksProvider>
  );
}
