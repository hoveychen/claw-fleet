import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  Activity,
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Circle,
  Folder,
  Inbox,
  MessageCircle,
  Search,
  SearchX,
  Square,
  WifiOff,
  X,
  XCircle,
} from "lucide-react";
import { EmptyState } from "./EmptyState";
import { SkeletonCard, Spinner } from "./loading";
import { useDelayedFlag } from "../useDelayedFlag";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { SessionInfo, SessionStatus } from "../types";
import { useDraft } from "../draft";
import { itemKey, type WithDevice } from "../deviceRuntime";
import { useRelaySearch } from "../useRelaySearch";
import { useConfirm } from "../confirmDialog";
import { useChatWorkspaces } from "../useChatWorkspace";
import { mentionPreview } from "../sessionMention";
import { repoRootPath } from "../../../shared-ts/repoPath";
import { AgentSourceIcon } from "./AgentSourceIcon";
import { compactDuration } from "../../../shared-ts/watchProgress";
import { createQuietLatch, stickyQuiet } from "../../../shared-ts/quietLatch";
import { createWakeLatch, isParkedForWake, stickyWaking } from "../../../shared-ts/wakeLatch";
import type { TaskState } from "../../../shared-ts/taskState";
import { speedTier } from "../../../shared-ts/taskSpeed";
import {
  buildTaskRows,
  projectScopeKey,
  projectSectionRows,
  rowDetail,
  activeSubtasks,
  runningSubagents,
  taskTokenSpeed,
  sectionRows,
  subtaskTally,
  type RowDetail,
  type TaskGroupBy,
  type TaskRow,
} from "../taskRows";
import { canControl, runStop, stopMode } from "./sessionStop";
import styles from "./TasksView.module.css";

/** Document-level scrollbar is shared by all tabs; the task view unmounts/remounts with
 *  the tab (see conditional rendering in App), and iOS PWA background/foreground switches
 *  plus reconnection full-state snapshots can reset window.scrollY to 0. This module
 *  remembers user scroll position and restores it on remount or accidental return to top,
 *  without fighting a scrolling user. Position stored in module state: tab switches don't
 *  reload JS so position persists; full page refresh naturally resets to zero, which is
 *  the right behavior. */
let savedTasksScrollY = 0;

/** Maximum scrollable distance on the page; <= 4px is considered "too short to need scrolling",
 *  so position is neither recorded nor restored. */
function maxScroll(): number {
  return document.documentElement.scrollHeight - window.innerHeight;
}

/** How long after scroll stops the sort order stays frozen. Scrolling itself resets this
 *  timer (every scroll event pushes it back), so the real meaning is "during scroll + 5s
 *  after user stops". */
const ORDER_FREEZE_MS = 5000;

/**
 * Preserve list order during the freeze window.
 *
 * The list sorts by descending activity, and the desktop pushes a full snapshot every few
 * seconds. While fingers are swiping on the list, a reorder swaps the row under the finger
 * for another—releasing and tapping opens the wrong task. `frozen` holds the key sequence
 * as it was at freeze time; null means unfrozen, pass-through.
 *
 * Rows appearing after freeze are appended to the end, not inserted at their sorted
 * position—they'd rank first by activity, shifting every row down, which is exactly the
 * tap-miss we avoid. Rows that vanished from the frozen key sequence are skipped.
 */
export function applyFrozenOrder<T>(
  rows: T[],
  frozen: readonly string[] | null,
  keyOf: (row: T) => string,
): T[] {
  if (!frozen || frozen.length === 0) return rows;
  const byKey = new Map<string, T>();
  for (const r of rows) byKey.set(keyOf(r), r);
  const out: T[] = [];
  const taken = new Set<string>();
  for (const key of frozen) {
    const r = byKey.get(key);
    if (!r) continue; // Already removed from the list
    out.push(r);
    taken.add(key);
  }
  for (const r of rows) {
    if (!taken.has(keyOf(r))) out.push(r);
  }
  return out;
}

const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];
const LIVE: SessionStatus[] = [...WORKING, "waitingInput", "active", "rateLimited", "serverErrored", "remoteDisconnected"];

/** Per-session status tone. Kept for its `"quiet"` verdict, which feeds the
 *  task-state derivation: the CLI process is still running while the scan-computed
 *  status has aged out to idle, because `determine_status` derives status from
 *  transcript age alone and a session parked on one long tool call stops writing.
 *
 *  The quiet tone is *latched* (`shared-ts/quietLatch.ts`, shared with the
 *  desktop row): without hysteresis the same session alternated working ↔ quiet
 *  several times a minute, because it writes one line every few minutes and
 *  each write pushes the status back to a live one for core's hard window. */
const quietLatch = createQuietLatch();

/** Holds a task out of 待验收 between being woken (card answered, watch
 *  fired, process started) and its transcript's first new line — see
 *  `shared-ts/wakeLatch.ts`, shared with the desktop home. */
const wakeLatch = createWakeLatch();

export function isWakingSticky(s: SessionInfo & { deviceId?: string }, pending: boolean): boolean {
  return stickyWaking(wakeLatch, `${s.deviceId ?? ""}/${s.id}`, {
    parked: isParkedForWake(s, pending),
    alive: !!s.procAlive,
    lastActivityMs: s.lastActivityMs ?? 0,
    now: Date.now(),
  });
}

export function statusTone(s: SessionInfo & { deviceId?: string }): string | null {
  const quiet = stickyQuiet(quietLatch, `${s.deviceId ?? ""}/${s.id}`, {
    alive: !!s.procAlive,
    rawQuiet: !!s.procAlive && !LIVE.includes(s.status),
    lastActivityMs: s.lastActivityMs ?? 0,
    now: Date.now(),
  });
  if (s.status === "watching") return "watching";
  if (s.status === "waitingInput") return "waiting";
  if (
    s.status === "rateLimited" ||
    s.status === "serverErrored" ||
    s.status === "remoteDisconnected" ||
    s.status === "stuck"
  )
    return "error";
  if (quiet) return "quiet";
  if (WORKING.includes(s.status)) return "working";
  if (s.status === "active") return "active";
  if (s.procAlive) return "quiet";
  return null;
}

export function timeAgo(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("刚刚");
  if (diff < 3_600_000) return t("{0} 分钟前", Math.floor(diff / 60_000));
  if (diff < 86_400_000) return t("{0} 小时前", Math.floor(diff / 3_600_000));
  return t("{0} 天前", Math.floor(diff / 86_400_000));
}

/** Section heading for a task state. Also the state's name wherever else a
 *  task's state is spoken of (the detail page's sub-task list). */
export function taskStateLabel(state: TaskState): string {
  if (state === "needsYou") return t("待你处理");
  if (state === "review") return t("待验收");
  if (state === "running") return t("进行中");
  if (state === "waiting") return t("等待中");
  return t("已完成");
}

/** A sub-task's state in words. A stopped sub-task has reported to the main
 *  conversation, not to the boss, so "review" reads as reported back. */
export function subtaskStateLabel(state: TaskState): string {
  return state === "review" ? t("已回报") : taskStateLabel(state);
}

/** The second line of a row, in words. */
function detailText(d: RowDetail): string | null {
  switch (d.kind) {
    case "subtaskNeedsYou":
      return t("来自子任务 {0} · 需要你决定", d.subtaskTitle);
    case "needsYou":
      return t("需要你决定");
    case "error":
      if (d.status === "rateLimited") return t("被限流了");
      if (d.status === "remoteDisconnected") return t("远端断开了");
      if (d.status === "stuck") return d.tool ? t("卡在 {0}", d.tool) : t("卡住了");
      return t("出错了 · 需要你看一下");
    case "outOfCredits":
      return t("额度耗尽");
    case "step": {
      const count = d.total ? t("步骤 {0}/{1}", d.done ?? 0, d.total) : null;
      return [count, d.text].filter(Boolean).join(" · ") || null;
    }
    case "wait":
      return t("等 {0}", d.reason);
    case "waitSubtasks":
      return t("等子任务 · {0} 个在跑", d.active);
    case "review":
      return t("已交付，等你验收");
    case "none":
      return null;
  }
}

/** FTS5 snippets arrive with literal `<mark>…</mark>` markers (see the desktop
 *  HistoryView). Split them into React nodes instead of trusting the transcript
 *  text as HTML. */
function renderSnippet(snippet: string): ReactNode[] {
  return snippet.split("<mark>").flatMap((chunk, i) => {
    if (i === 0) return [chunk];
    const end = chunk.indexOf("</mark>");
    if (end === -1) return [chunk];
    return [<mark key={i}>{chunk.slice(0, end)}</mark>, chunk.slice(end + "</mark>".length)];
  });
}

/** Sections folded when first seen: closed work is reference, not a to-do. */
const DEFAULT_COLLAPSED: TaskState[] = ["done"];

interface Props {
  /** Merged list: each session carries its device (WithDevice from deviceRuntime.ts).
   *  Id is unique only per-machine, so React key and "open this one" must include
   *  deviceId. */
  sessions: Array<WithDevice<SessionInfo>>;
  /** `itemKey(deviceId, sessionId)` of every session with a card waiting. */
  pendingKeys: ReadonlySet<string>;
  /** Transport for the device a session belongs to. Every write (accept) and every
   *  per-session lookup (search) must go to the session's own machine. */
  clientFor: (deviceId: string) => FleetTransport | null;
  /** WS link phone↔relay. */
  connected: boolean;
  /** Desktop↔relay link — false means nothing is there to push a snapshot. */
  agentOnline: boolean;
  /** Whether at least one `sessions` snapshot has arrived since connecting.
   *  Distinguishes "still waiting for the first push" from "pushed, but empty". */
  sessionsLoaded: boolean;
  /** Link up but no agent status reported yet (deviceRuntime.awaitingAgentReport):
   *  `agentOnline` is still its initial `false` and must not read as offline. */
  agentPending?: boolean;
  onOpenSession: (session: WithDevice<SessionInfo>) => void;
  /** Display label for this device. Prop absent = only one configured, so the
   *  device never enters a project name. */
  deviceLabelOf?: (deviceId: string) => string | null;
}

// The new-task entry point is owned by the raised button in the centre of App's
// bottom navigation; no duplicate on the task page.
export function TasksView({
  sessions,
  pendingKeys,
  clientFor,
  deviceLabelOf,
  connected,
  agentOnline,
  sessionsLoaded,
  agentPending = false,
  onOpenSession,
}: Props) {
  const confirm = useConfirm();
  // Persisted like a draft: tab switches unmount this view, and the boss should
  // come back to the same search and project.
  const [search, setSearch] = useDraft<string>("tasks:search", "");
  // Project scope: one project key, or "" for every project. A filter, not a
  // place — the list stays the same list, just narrower.
  const [projectScope, setProjectScope] = useDraft<string>("tasks:projectScope", "");
  const [collapsedStates, setCollapsedStates] = useDraft<TaskState[]>(
    "tasks:collapsedStates",
    DEFAULT_COLLAPSED,
  );
  const [groupBy, setGroupBy] = useDraft<TaskGroupBy>("tasks:groupBy", "status");
  // Project sections start expanded; folds are remembered per project.
  const [collapsedProjects, setCollapsedProjects] = useDraft<string[]>(
    "tasks:collapsedProjects",
    [],
  );
  // Accepted (marked done) rows move to the done section at once, reopened
  // rows leave it at once; the next snapshot confirms either.
  const [accepted, setAccepted] = useState<ReadonlySet<string>>(() => new Set());
  const [reopened, setReopened] = useState<ReadonlySet<string>>(() => new Set());
  const [accepting, setAccepting] = useState(false);
  // Row whose stop / interrupt is in flight (`itemKey` of its tip).
  const [stoppingKey, setStoppingKey] = useState<string | null>(null);

  const deviceIds = useMemo(() => [...new Set(sessions.map((s) => s.deviceId))], [sessions]);
  const { searching, ftsMatchKeys, snippetByKey } = useRelaySearch(deviceIds, clientFor, search);

  // Key sequence frozen during scroll (and ORDER_FREEZE_MS after), null = unfrozen.
  // State lets useMemos below recalculate; ref is the one source of truth for scroll
  // callbacks (callback closures don't see fresh state, but here we must immediately
  // know "already frozen" within the same scroll-event batch).
  const [frozenOrder, setFrozenOrder] = useState<string[] | null>(null);
  const frozenRef = useRef<string[] | null>(null);
  const renderedOrderRef = useRef<string[]>([]);

  const multiDevice = deviceLabelOf !== undefined;

  // Each device's pure-chat folder, pinned above every project. A device whose
  // path has not arrived (or an older desktop) just has no pinned chat.
  const chatPaths = useChatWorkspaces(deviceIds, clientFor);
  const chatKeys = useMemo(
    () =>
      new Set(
        Object.entries(chatPaths).map(([deviceId, path]) =>
          projectScopeKey(deviceId, repoRootPath(path), multiDevice),
        ),
      ),
    [chatPaths, multiDevice],
  );

  const rows = useMemo(() => {
    const built = buildTaskRows(sessions, {
      isPending: (deviceId, sessionId) => pendingKeys.has(itemKey(deviceId, sessionId)),
      isQuiet: (s) => statusTone(s) === "quiet",
      isWaking: (s, pending) => isWakingSticky(s, pending),
      multiDevice,
      deviceLabelOf,
      untitled: t("（无标题）"),
    }).map((r) => {
      // A pending card keeps the task under "needs you" whatever its outcome.
      if (accepted.has(r.key) && r.state !== "done" && !r.mainPending) {
        return { ...r, state: "done" as const, outcome: "completed" as const };
      }
      if (reopened.has(r.key) && r.state === "done") {
        return { ...r, state: "review" as const, outcome: null };
      }
      return r;
    });
    return applyFrozenOrder(built, frozenOrder, (r) => r.key);
  }, [sessions, pendingKeys, multiDevice, deviceLabelOf, accepted, reopened, frozenOrder]);

  // Drop optimistic entries the snapshot has caught up with, so a later change
  // made elsewhere (resumed on the desktop, closed from a card) shows through.
  useEffect(() => {
    const byKey = new Map(rows.map((r) => [r.key, r]));
    const closed = (k: string) => byKey.get(k)?.hops.some((h) => h.taskOutcome) ?? false;
    const keptAccepted = [...accepted].filter((k) => byKey.has(k) && !closed(k));
    if (keptAccepted.length !== accepted.size) setAccepted(new Set(keptAccepted));
    const keptReopened = [...reopened].filter((k) => byKey.has(k) && closed(k));
    if (keptReopened.length !== reopened.size) setReopened(new Set(keptReopened));
  }, [rows, accepted, reopened]);

  useEffect(() => {
    renderedOrderRef.current = rows.map((r) => r.key);
  }, [rows]);

  // Every project with a task, for the scope menu.
  const projectOptions = useMemo(() => {
    const by = new Map<string, { key: string; name: string; count: number }>();
    for (const r of rows) {
      const o = by.get(r.projectKey);
      if (o) o.count++;
      else by.set(r.projectKey, { key: r.projectKey, name: r.projectName, count: 1 });
    }
    return [...by.values()].sort(
      (a, b) => Number(chatKeys.has(b.key)) - Number(chatKeys.has(a.key)) || a.name.localeCompare(b.name),
    );
  }, [rows, chatKeys]);

  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    return rows.filter((r) => {
      if (projectScope && r.projectKey !== projectScope) return false;
      if (!q) return true;
      const members = [...r.hops, ...r.subtasks.flatMap((st) => st.hops)];
      return members.some(
        (s) =>
          `${s.titleOverride ?? ""} ${s.aiTitle ?? ""} ${s.slug ?? ""} ${s.lastMessagePreview ?? ""} ${s.workspaceName}`
            .toLowerCase()
            .includes(q) ||
          (s.taskPlan?.currentPlan?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentTask?.toLowerCase().includes(q) ?? false) ||
          ftsMatchKeys.has(itemKey(s.deviceId, s.jsonlPath)),
      );
    });
  }, [rows, projectScope, search, ftsMatchKeys]);

  const sections = useMemo(() => sectionRows(visible), [visible]);
  const projectSections = useMemo(
    () => (groupBy === "project" ? projectSectionRows(visible, (k) => chatKeys.has(k)) : []),
    [groupBy, visible, chatKeys],
  );

  const toggleSection = useCallback(
    (state: TaskState) =>
      setCollapsedStates((prev) =>
        prev.includes(state) ? prev.filter((s) => s !== state) : [...prev, state],
      ),
    [setCollapsedStates],
  );
  const toggleProject = useCallback(
    (key: string) =>
      setCollapsedProjects((prev) =>
        prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
      ),
    [setCollapsedProjects],
  );

  /** Accept tasks on their own machines; rows leave the section optimistically
   *  and come back if the desktop refuses. */
  const accept = useCallback(
    async (targets: Array<TaskRow<WithDevice<SessionInfo>>>) => {
      if (targets.length === 0) return;
      setAccepting(true);
      setAccepted((prev) => new Set([...prev, ...targets.map((r) => r.key)]));
      const byDevice = new Map<string, Array<TaskRow<WithDevice<SessionInfo>>>>();
      for (const r of targets) {
        const arr = byDevice.get(r.deviceId);
        if (arr) arr.push(r);
        else byDevice.set(r.deviceId, [r]);
      }
      await Promise.all(
        [...byDevice].map(async ([deviceId, group]) => {
          const transport = clientFor(deviceId);
          try {
            if (!transport) throw new Error("offline");
            await transport.request("accept_tasks", { sessionIds: group.map((r) => r.tip.id) });
          } catch {
            setAccepted((prev) => {
              const next = new Set(prev);
              for (const r of group) next.delete(r.key);
              return next;
            });
          }
        }),
      );
      setAccepting(false);
    },
    [clientFor],
  );

  /** Stop or interrupt a task's latest main hop, on its own machine: a pid
   *  only means something on the host that reported it. */
  const stop = useCallback(
    async (s: WithDevice<SessionInfo>) => {
      const transport = clientFor(s.deviceId);
      if (!transport || stoppingKey) return;
      setStoppingKey(itemKey(s.deviceId, s.id));
      try {
        await runStop(transport, s, confirm);
      } catch (e) {
        window.alert(e instanceof Error ? e.message : t("操作失败"));
      } finally {
        setStoppingKey(null);
      }
    },
    [clientFor, stoppingKey, confirm],
  );

  /** Reopen a closed task (改回进行中 / 撤销验收) on its own machine. */
  const reopen = useCallback(
    async (r: TaskRow<WithDevice<SessionInfo>>) => {
      setReopened((prev) => new Set([...prev, r.key]));
      setAccepted((prev) => {
        if (!prev.has(r.key)) return prev;
        const next = new Set(prev);
        next.delete(r.key);
        return next;
      });
      const transport = clientFor(r.deviceId);
      try {
        if (!transport) throw new Error("offline");
        await transport.request("reopen_tasks", { sessionIds: [r.tip.id] });
      } catch {
        setReopened((prev) => {
          const next = new Set(prev);
          next.delete(r.key);
          return next;
        });
      }
    },
    [clientFor],
  );

  const acceptAll = useCallback(
    async (targets: Array<TaskRow<WithDevice<SessionInfo>>>) => {
      if (!(await confirm(t("把这 {0} 个任务都标为已完成？", targets.length)))) return;
      await accept(targets);
    },
    [accept, confirm],
  );

  // ——— Scroll position stabilization (see savedTasksScrollY comment at file top) ———
  const restore = useCallback(() => {
    if (window.scrollY < 4 && savedTasksScrollY > 4 && maxScroll() >= savedTasksScrollY - 4) {
      window.scrollTo(0, savedTasksScrollY);
    }
  }, []);

  // Wait 150ms after scroll stops to record "user's true position". The same
  // listener manages sort-order freezing: the first scroll event snapshots the
  // current key sequence; each event thereafter pushes back the thaw timer.
  useEffect(() => {
    let idle: ReturnType<typeof setTimeout> | undefined;
    let thaw: ReturnType<typeof setTimeout> | undefined;
    const onScroll = () => {
      if (idle) clearTimeout(idle);
      idle = setTimeout(() => {
        if (maxScroll() > 4) savedTasksScrollY = window.scrollY;
      }, 150);
      if (frozenRef.current == null) {
        const snapshot = renderedOrderRef.current;
        frozenRef.current = snapshot;
        setFrozenOrder(snapshot);
      }
      if (thaw) clearTimeout(thaw);
      thaw = setTimeout(() => {
        frozenRef.current = null;
        setFrozenOrder(null);
      }, ORDER_FREEZE_MS);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      if (idle) clearTimeout(idle);
      if (thaw) clearTimeout(thaw);
    };
  }, []);

  useLayoutEffect(() => {
    if (savedTasksScrollY > 4 && maxScroll() >= savedTasksScrollY - 4) {
      window.scrollTo(0, savedTasksScrollY);
    }
  }, []);

  useLayoutEffect(() => {
    restore();
  }, [sessions, restore]);

  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") restore();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [restore]);

  // Distinguish reasons the list is empty so a blank screen never leaves users
  // guessing "is this broken or loading?": connectivity first, then whether the
  // first snapshot arrived, then a genuine "no tasks".
  const slowConnect = useDelayedFlag(rows.length === 0 && !connected, 6000);
  const listSkeleton = (
    <div className={styles.list} role="status" aria-busy="true" aria-label={t("正在加载任务…")}>
      {[0, 1, 2, 3, 4].map((i) => (
        <SkeletonCard key={i} height={64} />
      ))}
    </div>
  );
  if (rows.length === 0) {
    if (!connected) {
      return (
        <>
          {listSkeleton}
          {slowConnect && (
            <div className={styles.syncingHint}>
              {t("正在连接中转服务。若长时间停在这里，请检查手机网络。")}
            </div>
          )}
        </>
      );
    }
    if (!agentOnline && agentPending) return listSkeleton;
    if (!agentOnline) {
      return (
        <EmptyState
          icon={WifiOff}
          title={t("桌面端离线")}
          description={t("已连上中转，但桌面端 Fleet 未在线，暂时拿不到任务快照。请确认电脑上的 Fleet 正在运行。")}
        />
      );
    }
    if (!sessionsLoaded) return listSkeleton;
    return (
      <EmptyState
        icon={Inbox}
        title={t("还没有任务")}
        description={t("点下方 + 交代一件事，它会出现在这里。")}
      />
    );
  }

  const renderRow = (r: TaskRow<WithDevice<SessionInfo>>) => {
    const tip = r.tip;
    const snippet =
      search.trim().length >= 2
        ? [...r.hops, ...r.subtasks.flatMap((st) => st.hops)]
            .map((s) => snippetByKey.get(itemKey(s.deviceId, s.jsonlPath)))
            .find(Boolean)
        : undefined;
    const tally = subtaskTally(r);
    const line2 = [
      detailText(rowDetail(r, t("（无标题）"))) ??
        (r.state === "done" && r.outcome === "abandoned" ? t("已放弃") : null),
      tally ? t("子任务 {0}/{1}", tally.done, tally.total) : null,
    ]
      .filter(Boolean)
      .join(" · ");
    const live = r.state === "needsYou" || r.state === "running" || r.state === "waiting";
    const mode = stopMode(tip);
    const stoppable =
      live && canControl(tip) && mode !== "spent" && clientFor(tip.deviceId) !== null;
    const stopBusy = stoppingKey === itemKey(tip.deviceId, tip.id);
    const startedAt = tip.taskSummary?.startedAtMs || Math.min(...r.hops.map((h) => h.createdAtMs));
    const subagents = runningSubagents(r);
    const liveSubtasks = activeSubtasks(r);
    const helpersLabel = [
      liveSubtasks > 0 ? t("{0} 个子任务在跑", liveSubtasks) : null,
      subagents > 0 ? t("{0} 个子代理在跑", subagents) : null,
    ]
      .filter(Boolean)
      .join(" · ");
    const preview = tip.lastMessagePreview ? mentionPreview(tip.lastMessagePreview) : "";
    return (
      <div key={r.key} className={styles.row} onClick={() => onOpenSession(tip)}>
        <button
          className={styles.markToggle}
          data-state={r.state}
          data-outcome={r.state === "done" ? (r.outcome ?? "completed") : undefined}
          data-speed={r.state === "done" ? undefined : speedTier(taskTokenSpeed(r))}
          aria-pressed={r.state === "done"}
          aria-label={
            r.state !== "done"
              ? t("进行中 — 点击标为已完成")
              : r.outcome === "abandoned"
                ? t("已放弃 — 点击改回进行中")
                : t("已完成 — 点击改回进行中")
          }
          onClick={(e) => {
            e.stopPropagation();
            if (r.state === "done") void reopen(r);
            else void accept([r]);
          }}
        >
          {r.state !== "done" ? (
            <Circle size={16} />
          ) : r.outcome === "abandoned" ? (
            <XCircle size={16} />
          ) : (
            <CheckCircle2 size={16} />
          )}
        </button>
        <div className={styles.rowBody}>
          <div className={styles.line1}>
            <span className={styles.title}>{r.title}</span>
          </div>
          {/* Source mark, subagent count and project tag ride on the second
              line, so the title keeps all of the first even beside a stop
              button on a 390px screen. */}
          <div className={styles.line2}>
            <span className={styles.sourceIcon} title={tip.agentSource || "claude-code"}>
              <AgentSourceIcon source={tip.agentSource} />
            </span>
            {line2 && <span className={styles.line2Text}>{line2}</span>}
            {subagents + liveSubtasks > 0 && (
              <span className={styles.subagents} aria-label={helpersLabel}>
                <Bot size={11} />
                {subagents + liveSubtasks}
              </span>
            )}
            {/* Under a project heading the tag would only repeat it. */}
            {!projectScope && groupBy === "status" && (
              <span className={styles.project}>{r.projectName}</span>
            )}
          </div>
          {snippet ? (
            <div className={styles.snippet}>{renderSnippet(snippet)}</div>
          ) : (
            preview && <div className={styles.preview}>{preview}</div>
          )}
        </div>
        {stoppable && (
          <button
            className={styles.stopButton}
            data-mode={mode}
            disabled={stopBusy}
            aria-label={mode === "interrupt" ? t("中断当前回合") : t("停止")}
            onClick={(e) => {
              e.stopPropagation();
              void stop(tip);
            }}
          >
            {stopBusy ? <Spinner size={12} /> : <Square size={11} />}
            {mode === "interrupt" ? t("中断") : t("停止")}
          </button>
        )}
        {r.state === "review" ? (
          <button
            className={styles.acceptButton}
            disabled={accepting}
            onClick={(e) => {
              e.stopPropagation();
              void accept([r]);
            }}
          >
            {t("验收")}
          </button>
        ) : stoppable ? null : (
          <span className={styles.time}>
            {live ? compactDuration(Date.now() - startedAt) : timeAgo(r.lastActivityMs)}
          </span>
        )}
        <ChevronRight size={16} className={styles.chevron} aria-hidden="true" />
      </div>
    );
  };

  return (
    <div className={styles.wrapper}>
      <div className={styles.pageHead}>
        <h1 className={styles.pageTitle}>{t("任务")}</h1>
        <div className={styles.segment} role="group" aria-label={t("分组方式")}>
          {(["status", "project"] as TaskGroupBy[]).map((mode) => {
            const label = mode === "status" ? t("按状态分组") : t("按项目分组");
            return (
              <button
                key={mode}
                className={styles.segmentButton}
                data-active={groupBy === mode}
                aria-pressed={groupBy === mode}
                aria-label={label}
                title={label}
                onClick={() => setGroupBy(mode)}
              >
                {mode === "status" ? <Activity size={15} /> : <Folder size={15} />}
              </button>
            );
          })}
        </div>
        {/* A native select: the phone's own picker handles a long project list
            better than any custom popover. */}
        <label className={styles.scopeSelectWrap} data-on={!!projectScope}>
          <select
            className={styles.scopeSelect}
            value={projectScope}
            onChange={(e) => setProjectScope(e.target.value)}
            aria-label={t("全部项目")}
          >
            <option value="">{t("全部项目")}</option>
            {projectOptions.map((p) => (
              <option key={p.key} value={p.key}>
                {`${p.name} (${p.count})`}
              </option>
            ))}
            {/* A scope whose project has left the list still shows, so it can be cleared. */}
            {projectScope && !projectOptions.some((p) => p.key === projectScope) && (
              <option value={projectScope}>
                {projectScope.split(/[\\/]/).filter(Boolean).pop() ?? projectScope}
              </option>
            )}
          </select>
          <ChevronDown size={14} aria-hidden="true" />
        </label>
        {projectScope && (
          <button
            className={styles.scopeClear}
            onClick={() => setProjectScope("")}
            aria-label={t("显示全部项目")}
          >
            <X size={14} strokeWidth={2.2} />
          </button>
        )}
      </div>

      <div className={styles.searchWrap}>
        <span className={styles.searchIcon}>
          <Search size={14} />
        </span>
        <input
          className={styles.search}
          type="search"
          placeholder={t("搜索任务、步骤、全文…")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        {searching && (
          <span className={styles.searchSpinner}>
            <Spinner size={13} />
          </span>
        )}
      </div>

      {!sessionsLoaded && (
        <div className={styles.syncingHint}>
          <Spinner size={12} />
          {t("显示上次缓存，正在同步…")}
        </div>
      )}

      {visible.length === 0 && searching && (
        <div className={styles.searchPending}>
          <Spinner size={16} label={t("搜索中…")} />
        </div>
      )}
      {visible.length === 0 && !searching && (
        <EmptyState compact icon={SearchX} title={t("没有匹配的任务")} />
      )}

      <div className={styles.list}>
        {projectSections.map((sec) => {
          const collapsed = !search.trim() && !projectScope && collapsedProjects.includes(sec.key);
          return (
            <section key={sec.key} className={styles.section}>
              <div className={styles.sectionHead}>
                <button
                  className={styles.sectionToggle}
                  aria-expanded={!collapsed}
                  onClick={() => toggleProject(sec.key)}
                >
                  {sec.chat ? (
                    <MessageCircle size={14} className={styles.sectionIcon} />
                  ) : (
                    <Folder size={14} className={styles.sectionIcon} />
                  )}
                  <span className={styles.sectionName}>{sec.chat ? t("纯聊天") : sec.name}</span>
                  <span className={styles.sectionCount}>{sec.rows.length}</span>
                  {sec.needsYou > 0 && (
                    <span className={styles.needsYouBadge}>{t("需要你 {0}", sec.needsYou)}</span>
                  )}
                </button>
                <button
                  className={styles.sectionChevron}
                  data-open={!collapsed}
                  aria-label={collapsed ? t("展开") : t("收起")}
                  onClick={() => toggleProject(sec.key)}
                >
                  <ChevronDown size={16} />
                </button>
              </div>
              {!collapsed && sec.rows.map(renderRow)}
            </section>
          );
        })}
        {groupBy === "status" && sections.map((sec) => {
          // A search or project scope means the boss is looking for something;
          // a leftover fold would hide it.
          const collapsed =
            !search.trim() && !projectScope && collapsedStates.includes(sec.state);
          const reviewRows = sec.state === "review" ? sec.rows : [];
          return (
            <section key={sec.state} className={styles.section}>
              <div className={styles.sectionHead}>
                <button
                  className={styles.sectionToggle}
                  aria-expanded={!collapsed}
                  onClick={() => toggleSection(sec.state)}
                >
                  <span className={styles.sectionName}>{taskStateLabel(sec.state)}</span>
                  <span className={styles.sectionCount}>{sec.rows.length}</span>
                </button>
                {reviewRows.length > 1 && (
                  <button
                    className={styles.acceptAll}
                    disabled={accepting}
                    onClick={() => void acceptAll(reviewRows)}
                  >
                    {t("全部验收")}
                  </button>
                )}
                <button
                  className={styles.sectionChevron}
                  data-open={!collapsed}
                  aria-label={collapsed ? t("展开") : t("收起")}
                  onClick={() => toggleSection(sec.state)}
                >
                  <ChevronDown size={16} />
                </button>
              </div>
              {!collapsed && sec.rows.map(renderRow)}
            </section>
          );
        })}
      </div>
    </div>
  );
}
