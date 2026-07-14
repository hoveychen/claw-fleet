import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import {
  Bot,
  CheckCheck,
  CheckCircle2,
  Circle,
  Clock,
  FolderGit2,
  History,
  Plus,
  Waypoints,
} from "lucide-react";
import { useReadStore, useSessionsStore, useUIStore, type MarkFilter } from "../store";
import type { SessionInfo } from "../types";
import { LIVE_STATUSES, isFleetOwnedEntrypoint, rowBarColor, sessionUnread } from "../types";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { PageShell } from "./PageShell";
import {
  NewSessionForm,
  distinctWorkspaces,
  repoRootPath,
  type NewSessionCreated,
} from "./NewSessionForm";
import { SessionDetail } from "./SessionDetail";
import { SessionTabs } from "./SessionTabs";
import {
  closeOtherTabs as closeOtherTabsState,
  closeTab as closeTabState,
  closeTabsToRight as closeTabsToRightState,
  openTab as openTabState,
  parsePersistedTabs,
  pruneMissingTabs,
  reorderTabs as reorderTabIds,
  TABS_STORAGE_KEY,
  type TabState,
} from "../sessionTabs";
import { getItem, setItem } from "../storage";
import { StopControl, canControl } from "./StopControl";
import { MarkControl } from "./MarkControl";
import styles from "./HistoryView.module.css";

/** A session spawned but not yet discovered by the scanner. We poll the session
 *  list for the matching `SessionInfo` and swap the detail column over to it. */
interface PendingSpawn extends NewSessionCreated {
  /** Ad-hoc session ids that already existed when the spawn returned. The new
   *  session is the one whose id is NOT in this set — pid can't identify it
   *  (see `matchSpawnedSession`). */
  knownIds: Set<string>;
}

/** Give up on the in-place spinner after this long and offer an escape hatch;
 *  the scanner polls every ~5s so a healthy spawn resolves well within this. */
const START_TIMEOUT_MS = 30_000;

/**
 * Pick the freshly-spawned session out of the scanned list.
 *
 * The spawn pre-assigns the session id via `--session-id` and returns it, so
 * the primary match is a direct id lookup. The novelty fallback (the ad-hoc
 * session in the target workspace whose id was absent when the spawn returned;
 * most-recently-created wins) covers older remote probes that only return the
 * pid. We still deliberately do NOT match on pid — see `resolve_pid` in
 * session.rs: a cwd-shared pid can tag several sessions at once.
 */
function matchSpawnedSession(
  adhocSessions: SessionInfo[],
  pending: PendingSpawn,
): SessionInfo | undefined {
  if (pending.sessionId) {
    return adhocSessions.find((s) => s.id === pending.sessionId);
  }
  return adhocSessions
    .filter(
      (s) => s.workspacePath === pending.workspacePath && !pending.knownIds.has(s.id),
    )
    .sort((a, b) => b.createdAtMs - a.createdAtMs)[0];
}

/** Segments for the pending/done filter. "all" shows everything; the other two
 *  map to the binary mark buckets (unmarked collapses to "pending"). The
 *  read/unread axis is deliberately NOT filterable — only pending/done is.
 *
 *  The two bucket segments render the exact icons `MarkControl` puts on the row
 *  (hollow circle = pending, green check = done) so a segment reads as "show me
 *  the rows wearing this icon". "all" is spelled out in words instead — any
 *  third icon here just competes with those two for meaning.
 *
 *  The selected segment itself lives in `useUIStore` (and on disk), not in this
 *  component — see the `history*` fields there for why. */
const MARK_SEGMENTS: MarkFilter[] = ["all", "pending", "done"];

/** Which bucket a session falls in — `done` is explicit, everything else is
 *  pending. */
function markBucket(s: SessionInfo): "pending" | "done" {
  return s.userMark === "done" ? "done" : "pending";
}

/** The two pseudo-workspaces the workspace `<select>` pins above the real
 *  directories. Every real value is an absolute path, so these bare words can
 *  never collide with one — including on Windows, where a path starts with a
 *  drive letter. They are persisted like any other filter value. */
export const CHAT_ONLY = "chat";
export const CHAT_HIDDEN = "no-chat";

/** Does `s` pass the workspace filter? `chatPath` is the pure-chat workspace,
 *  `null` while the backend call is in flight or if it failed — in which case
 *  the two chat pseudo-values degrade to "all" rather than silently emptying
 *  the rail. */
export function matchesWorkspaceFilter(
  s: SessionInfo,
  filter: string,
  chatPath: string | null,
): boolean {
  if (filter === "all") return true;
  if (filter === CHAT_ONLY) return chatPath == null || s.workspacePath === chatPath;
  if (filter === CHAT_HIDDEN) return chatPath == null || s.workspacePath !== chatPath;
  // Options are collapsed to repo roots (see the dropdown's distinctWorkspaces),
  // so a session inside `<repo>/.worktrees/<task>` matches its root's option.
  return repoRootPath(s.workspacePath) === filter;
}

function timeAgo(ms: number, t: (k: string, opts?: Record<string, unknown>) => string): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("just_now");
  if (diff < 3_600_000) return t("m_ago", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("h_ago", { n: Math.floor(diff / 3_600_000) });
  return t("d_ago", { n: Math.floor(diff / 86_400_000) });
}

/** Elapsed run time = now − session start (createdAtMs). Shown only for live
 *  rows (busy or waiting-input), where "how long has this been going" is the
 *  useful signal. Same single-unit shape as timeAgo, but counts up from start
 *  instead of down from last activity. */
function formatRunning(ms: number, t: (k: string, opts?: Record<string, unknown>) => string): string {
  const diff = Math.max(0, Date.now() - ms);
  if (diff < 60_000) return t("ran_s", { n: Math.floor(diff / 1_000) });
  if (diff < 3_600_000) return t("ran_m", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("ran_h", { n: Math.floor(diff / 3_600_000) });
  return t("ran_d", { n: Math.floor(diff / 86_400_000) });
}

/** Rows keep their control while there is a live process to aim it at; once the
 *  session is spent the control renders disabled rather than vanishing, so the
 *  row doesn't reflow under the cursor mid-click. */
function showsControl(s: SessionInfo): boolean {
  return canControl(s) && (s.pid !== null || LIVE_STATUSES.has(s.status));
}

/** FTS5 snippets arrive with literal `<mark>…</mark>` markers (see
 *  search_index.rs). Split them into React nodes instead of trusting the
 *  transcript text as HTML. */
function renderSnippet(snippet: string): ReactNode[] {
  return snippet.split("<mark>").flatMap((chunk, i) => {
    if (i === 0) return [chunk];
    const end = chunk.indexOf("</mark>");
    if (end === -1) return [chunk];
    return [
      <mark key={i}>{chunk.slice(0, end)}</mark>,
      chunk.slice(end + "</mark>".length),
    ];
  });
}

/**
 * Compare two SessionInfo by value. The scanner re-emits the whole list every
 * couple of seconds and each entry is a fresh JSON deserialisation, so object
 * identity always differs even when nothing changed — a reference check would
 * make SessionRow's memo useless.
 *
 * Deliberately keyed off the object's own keys rather than a hand-maintained
 * field list: a new SessionInfo field is then covered automatically, instead of
 * silently freezing a row that a forgotten list entry would have updated.
 * Scalars compare with `===`; the few nested fields (handoff, taskPlan, todos)
 * fall back to a structural compare — they are small.
 */
export function sessionEq(a: SessionInfo, b: SessionInfo): boolean {
  if (a === b) return true;
  const keys = Object.keys(a) as (keyof SessionInfo)[];
  if (keys.length !== Object.keys(b).length) return false;
  for (const k of keys) {
    const va = a[k];
    const vb = b[k];
    if (va === vb) continue;
    if (va && vb && typeof va === "object" && typeof vb === "object") {
      if (JSON.stringify(va) !== JSON.stringify(vb)) return false;
      continue;
    }
    return false;
  }
  return true;
}

type SessionRowProps = {
  session: SessionInfo;
  snippet: string | undefined;
  isSelected: boolean;
  /** Open in a tab, but not the tab currently on screen. Weaker half of the
   *  same axis as `isSelected` — never both at once. */
  isOpen: boolean;
  unread: boolean;
  /** Bumped every 30s by the parent so relative times keep advancing. Without
   *  it the memo would freeze the elapsed "3 分钟" at whatever it said on mount. */
  nowTick: number;
  onClick: (s: SessionInfo) => void;
};

const SessionRow = memo(function SessionRow({
  session: s,
  snippet,
  isSelected,
  isOpen,
  unread,
  onClick,
}: SessionRowProps) {
  const { t } = useTranslation();
  const runColor = rowBarColor(s);
  // Reveal-on-hover for the stop button is driven by this JS state, not CSS
  // `:hover`: wry/WebKit fails to recompute `:hover` when a row reflows without a
  // fresh pointer event, so a CSS-only reveal could stick on after the cursor
  // left. State set only by real enter/leave events can't latch across
  // re-renders; `window blur` clears it if the pointer is yanked out of the
  // window without a mouseleave. (Same fix as SessionCard.)
  const [hovered, setHovered] = useState(false);
  useEffect(() => {
    if (!hovered) return;
    const clear = () => setHovered(false);
    window.addEventListener("blur", clear);
    return () => window.removeEventListener("blur", clear);
  }, [hovered]);
  return (
    <div
      className={`${styles.row_wrap} ${hovered ? styles.hovered : ""}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <button
        type="button"
        className={`${styles.row} ${isSelected ? styles.row_active : isOpen ? styles.row_open : ""} ${unread ? styles.row_unread : ""}`}
        onClick={() => onClick(s)}
        title={s.lastMessagePreview ?? undefined}
        aria-label={unread ? t("history.unread", "未读 — 有新消息") : undefined}
      >
        {runColor && (
          <span
            className={styles.row_status_dot}
            style={{ background: runColor }}
            title={
              s.status === "waitingInput"
                ? t("history.waiting", "等待输入")
                : t("history.running", "运行中")
            }
          />
        )}
        <span className={styles.row_body}>
          <span className={`${styles.row_title} ${unread ? styles.row_title_unread : ""}`}>
            {s.aiTitle ?? s.slug ?? s.lastMessagePreview ?? t("history.untitled", "（无标题）")}
          </span>
          <span className={styles.row_meta}>
            <span className={styles.row_project} title={s.workspacePath}>
              <FolderGit2 size={10} strokeWidth={1.6} />
              {s.workspaceName}
            </span>
            {s.handoff && (
              <span
                className={styles.row_handoff}
                title={t("card.tip_handoff", {
                  hop: s.handoff.hop,
                  len: s.handoff.chainLen,
                })}
              >
                <Waypoints size={10} strokeWidth={1.6} />
                {s.handoff.hop}/{s.handoff.chainLen}
              </span>
            )}
            {LIVE_STATUSES.has(s.status) && (
              <span
                className={styles.row_runtime}
                style={{ color: runColor ?? undefined }}
                title={new Date(s.createdAtMs).toLocaleString()}
              >
                <Clock size={10} strokeWidth={1.6} />
                {formatRunning(s.createdAtMs, t)}
              </span>
            )}
            {s.runningSubagentCount > 0 && (
              <span
                className={styles.row_subagents}
                style={{ color: runColor ?? undefined }}
                title={t("history.running_subagents", {
                  count: s.runningSubagentCount,
                })}
              >
                <Bot size={10} strokeWidth={1.6} />
                {s.runningSubagentCount}
              </span>
            )}
            {/* Activity time uses the aggregate (own ∪ subagents): a session
                delegating to subagents keeps a stale own `lastActivityMs` while
                the tree churns, so this reflects the whole tree being alive. */}
            <span
              className={styles.row_time}
              title={
                s.agentLastActivityMs !== s.lastActivityMs
                  ? t("history.activity_via_subagent")
                  : undefined
              }
            >
              {timeAgo(s.agentLastActivityMs, t)}
            </span>
          </span>
          {snippet && <span className={styles.row_snippet}>{renderSnippet(snippet)}</span>}
        </span>
      </button>
      <span className={styles.row_actions}>
        {showsControl(s) && (
          <span className={styles.stop_slot}>
            <StopControl session={s} />
          </span>
        )}
        <MarkControl session={s} />
      </span>
    </div>
  );
},
(prev, next) =>
  prev.isSelected === next.isSelected &&
  prev.isOpen === next.isOpen &&
  prev.unread === next.unread &&
  prev.snippet === next.snippet &&
  prev.nowTick === next.nowTick &&
  prev.onClick === next.onClick &&
  sessionEq(prev.session, next.session));

/**
 * History page: sessions launched via the "新会话" button, as a master-detail
 * view — left rail lists the sessions (text search + workspace filter),
 * clicking a row renders that session's SessionDetail inline on the right.
 *
 * Zero bookkeeping: those spawns carry `CLAUDE_CODE_ENTRYPOINT` (see
 * session_launch::NEW_SESSION_ENTRYPOINT), which the Claude CLI persists into
 * the transcript itself, so the regular scan (`SessionInfo.entrypoint`) is the
 * data source — same mechanism the VS Code extension uses. History therefore
 * lives exactly as long as the transcript on disk, within the scanner's
 * 7-day window.
 */
export function HistoryView() {
  const { t } = useTranslation();
  // Subscribe per-field: the store also carries speedHistory/costHistory, which
  // grow on every scan tick. Destructuring the whole slice would re-render the
  // launchpad on that churn even when the session list itself is unchanged.
  const sessions = useSessionsStore((s) => s.sessions);
  const scanReady = useSessionsStore((s) => s.scanReady);
  // Read/unread axis — optimistic overrides hide the dot before the next scan
  // re-stamps `lastReadMs`; see useReadStore.
  const readOverrides = useReadStore((s) => s.overrides);
  const markRead = useReadStore((s) => s.markRead);
  const markManyRead = useReadStore((s) => s.markManyRead);

  // Rail filters live in the store, not here: this component is unmounted every
  // time `viewMode` leaves "history", which would otherwise reset them behind
  // the user's back (a waiting-input alert forcing setViewMode("list") is enough).
  const query = useUIStore((s) => s.historyQuery);
  const setQuery = useUIStore((s) => s.setHistoryQuery);
  const workspaceFilter = useUIStore((s) => s.historyWorkspaceFilter);
  const setWorkspaceFilter = useUIStore((s) => s.setHistoryWorkspaceFilter);
  // Re-render on a slow tick so the relative "last updated" and the live
  // Elapsed-runtime durations keep counting even when no scan lands (a waiting-input
  // session can sit idle for minutes). 30s granularity matches the minute-level
  // display without churning the list.
  const [nowTick, setNowTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setNowTick((n) => n + 1), 30_000);
    return () => clearInterval(id);
  }, []);
  // Narrow the rail to sessions whose agent is still live — same status set that
  // colours the row dot green/amber.
  const activeOnly = useUIStore((s) => s.historyActiveOnly);
  const setActiveOnly = useUIStore((s) => s.setHistoryActiveOnly);
  // Segmented filter by manual review mark; "all" shows every bucket.
  const markFilter = useUIStore((s) => s.historyMarkFilter);
  const setMarkFilter = useUIStore((s) => s.setHistoryMarkFilter);
  // Inline detail column selection — local to the page, deliberately NOT the
  // global useDetailStore (that one drives the drawer overlaying every view).
  //
  // The column is an IDE-style tab strip: `tabIds` is the open tabs left→right
  // and `activeId` is the one on screen. We keep *ids*, not SessionInfo
  // snapshots, and resolve them against the live scan on every render — a tab
  // that has been open for ten minutes must show the session's current title
  // and status, not the ones it wore when it was opened.
  //
  // Restored from the previous run. The lazy initialiser matters: `initStorage`
  // is awaited before React renders, so reading at first render sees a warm
  // cache, while a module-level read would run at import time and see nothing.
  const [restored] = useState(() =>
    parsePersistedTabs(getItem(TABS_STORAGE_KEY)),
  );
  const [tabIds, setTabIds] = useState<string[]>(restored.tabIds);
  const [activeId, setActiveId] = useState<string | null>(restored.activeId);
  // Per-tab search highlight (the FTS query that matched that session), keyed
  // by session id — each tab was opened by its own click and carries its own.
  const [queryById, setQueryById] = useState<Record<string, string | null>>({});
  // Detail-column modes are mutually exclusive, in precedence order:
  //   pending  → "starting…" spinner while we wait for the scanner
  //   composing→ the inline new-session form
  //   tabs     → the open sessions' SessionDetail panes
  //   (none)   → the empty-state hint
  const [composing, setComposing] = useState(false);
  const [pending, setPending] = useState<PendingSpawn | null>(null);
  const [startTimedOut, setStartTimedOut] = useState(false);

  const { searching, ftsMatchPaths, snippetByPath } = useSessionSearch(query);

  const adhocSessions = useMemo(
    () =>
      sessions.filter(
        (s) => !s.isSubagent && isFleetOwnedEntrypoint(s.entrypoint),
      ),
    [sessions],
  );

  // The pure-chat workspace is filtered through its own pinned options, so it
  // is kept out of the project list below — otherwise it would sit in there a
  // second time, alphabetised among the repos as plain "Chat".
  const chatPath = useChatWorkspace();

  // Same derivation as the New Session launcher (distinctWorkspaces): temp
  // scratchpad cwds dropped, in-repo worktree checkouts folded onto their repo
  // root. No cap here — the filter should list every real directory — so the
  // matcher above compares on repoRootPath to keep folded options selectable.
  const workspaces = useMemo(
    () =>
      distinctWorkspaces(adhocSessions, Number.MAX_SAFE_INTEGER, chatPath)
        .map((w) => [w.path, w.name] as [string, string])
        .sort((a, b) => a[1].localeCompare(b[1])),
    [adhocSessions, chatPath],
  );

  // A filter persisted before the chat workspace got its own option is a raw
  // chat path; it no longer appears among the project options, so promote it to
  // the pseudo-value instead of letting the reset below silently drop it.
  useEffect(() => {
    if (chatPath && workspaceFilter === chatPath) setWorkspaceFilter(CHAT_ONLY);
  }, [chatPath, workspaceFilter, setWorkspaceFilter]);

  // The workspace filter is persisted, but the dropdown's options only cover
  // workspaces with sessions inside the scanner's 7-day window. A restored path
  // whose sessions have aged out would render a blank <select> over an empty
  // rail, with no obvious way back — fall back to "all" once the scan lands.
  // The two chat pseudo-values are always offered, so they never age out.
  useEffect(() => {
    if (!scanReady || workspaceFilter === "all") return;
    if (workspaceFilter === CHAT_ONLY || workspaceFilter === CHAT_HIDDEN) return;
    if (workspaceFilter === chatPath) return; // promoted by the effect above
    if (!workspaces.some(([path]) => path === workspaceFilter)) {
      setWorkspaceFilter("all");
    }
  }, [scanReady, workspaces, workspaceFilter, chatPath, setWorkspaceFilter]);

  const { rows, markCounts } = useMemo(() => {
    const q = query.trim().toLowerCase();
    // Everything except the mark filter — the segment counts are taken over
    // this set so each count reflects how many rows its segment would reveal
    // under the current workspace / query / active filters.
    const preMark = adhocSessions
      .filter((s) => matchesWorkspaceFilter(s, workspaceFilter, chatPath))
      .filter((s) => !activeOnly || LIVE_STATUSES.has(s.status))
      .filter((s) => {
        if (!q) return true;
        const clientMatch =
          (s.aiTitle?.toLowerCase().includes(q) ?? false) ||
          (s.slug?.toLowerCase().includes(q) ?? false) ||
          (s.lastMessagePreview?.toLowerCase().includes(q) ?? false) ||
          s.workspaceName.toLowerCase().includes(q) ||
          // The attributed TASKS.md plan — id (`scene-items`), title, and the
          // current P-task. Sessions pulled onto a plan by `fleet handoff`
          // inherit the attribution, so the whole relay chain is reachable by
          // searching the plan the way a human names it.
          (s.taskPlan?.planId?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentPlan?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentTask?.toLowerCase().includes(q) ?? false);
        return clientMatch || ftsMatchPaths.has(s.jsonlPath);
      });
    const counts: Record<MarkFilter, number> = {
      all: preMark.length,
      pending: 0,
      done: 0,
    };
    for (const s of preMark) counts[markBucket(s)] += 1;
    const rows = preMark
      .filter((s) => markFilter === "all" || markBucket(s) === markFilter)
      // Most recently *active* first, matching the row's displayed time. Uses
      // the aggregate activity (own ∪ subagents) so a session that is quietly
      // driving a subagent swarm doesn't sink below idle rows — its own
      // `lastActivityMs` would be stale, but the tree is very much alive.
      .sort((a, b) => b.agentLastActivityMs - a.agentLastActivityMs);
    return { rows, markCounts: counts };
  }, [adhocSessions, workspaceFilter, chatPath, activeOnly, query, ftsMatchPaths, markFilter]);

  const activeCount = useMemo(
    () => adhocSessions.filter((s) => LIVE_STATUSES.has(s.status)).length,
    [adhocSessions],
  );

  // Every launchpad session with newer activity than its last read — drives the
  // "一键清除未读" button. Scoped to all adhoc sessions (not the filtered rows) so
  // the button truly zeroes the unread count / sidebar badge.
  const unreadSessions = useMemo(
    () => adhocSessions.filter((s) => sessionUnread(s, readOverrides[s.id])),
    [adhocSessions, readOverrides],
  );

  // Open tabs, resolved against the live scan. An id whose session has vanished
  // from the scan resolves to nothing and simply drops out of the strip; we
  // deliberately do NOT prune `tabIds` for it, so a session that blips out of a
  // scan cycle comes back to its tab rather than being silently closed.
  const sessionById = useMemo(
    () => new Map(sessions.map((s) => [s.id, s])),
    [sessions],
  );
  const tabs = useMemo(
    () =>
      tabIds
        .map((id) => sessionById.get(id))
        .filter((s): s is SessionInfo => s != null),
    [tabIds, sessionById],
  );

  // Compose / pending own the whole column while they're up. The tab panes stay
  // mounted underneath (hidden + paused) so cancelling out of the composer puts
  // the user back on the tab they left, exactly where they left it.
  const overlaid = pending != null || composing;

  // Persist the strip so it comes back on the next launch. The per-tab search
  // highlight is deliberately NOT persisted — a term you searched for last week
  // has no business highlighting a transcript on a cold start.
  useEffect(() => {
    setItem(TABS_STORAGE_KEY, JSON.stringify({ tabIds, activeId }));
  }, [tabIds, activeId]);

  // Restored ids can name sessions that have since been deleted. They already
  // render as nothing (they don't resolve against the scan), but left in the
  // list they'd persist forever and grow. Prune once, against the first scan
  // that lands — before it, `sessions` is empty and pruning would close
  // everything.
  const prunedRef = useRef(false);
  useEffect(() => {
    if (!scanReady || prunedRef.current) return;
    prunedRef.current = true;
    const next = pruneMissingTabs({ tabIds, activeId }, (id) => sessionById.has(id));
    if (next.tabIds.length === tabIds.length) return;
    setTabIds(next.tabIds);
    setActiveId(next.activeId);
  }, [scanReady, sessionById, tabIds, activeId]);

  // Clicking a tab also dismisses whatever overlay was covering the column, so
  // the strip doubles as the way back out of the composer.
  const activateTab = useCallback((id: string) => {
    setComposing(false);
    setPending(null);
    setStartTimedOut(false);
    setActiveId(id);
  }, []);

  // The close/reorder rules (chiefly: where focus lands after a close) live in
  // `sessionTabs.ts` as pure reducers so they can be unit-tested.
  const applyTabs = useCallback(
    (fn: (state: TabState) => TabState) => {
      const next = fn({ tabIds, activeId });
      setTabIds(next.tabIds);
      setActiveId(next.activeId);
      // Drop highlights for tabs that are no longer open.
      setQueryById((prev) =>
        Object.fromEntries(
          Object.entries(prev).filter(([id]) => next.tabIds.includes(id)),
        ),
      );
    },
    [tabIds, activeId],
  );

  const openTab = useCallback(
    (s: SessionInfo, highlight: string | null) => {
      setComposing(false);
      setPending(null);
      setStartTimedOut(false);
      applyTabs((st) => openTabState(st, s.id));
      setQueryById((prev) => ({ ...prev, [s.id]: highlight }));
    },
    [applyTabs],
  );

  // Stable identity: SessionRow is memoised, and a fresh closure each render
  // would defeat it for every row.
  const handleRowClick = useCallback(
    (s: SessionInfo) => {
      const isFtsHit = query.trim().length >= 2 && ftsMatchPaths.has(s.jsonlPath);
      openTab(s, isFtsHit ? query.trim() : null);
    },
    [query, ftsMatchPaths, openTab],
  );

  const closeTab = useCallback(
    (id: string) => applyTabs((s) => closeTabState(s, id)),
    [applyTabs],
  );
  const closeOtherTabs = useCallback(
    (id: string) => applyTabs((s) => closeOtherTabsState(s, id)),
    [applyTabs],
  );
  const closeTabsToRight = useCallback(
    (id: string) => applyTabs((s) => closeTabsToRightState(s, id)),
    [applyTabs],
  );
  const closeAllTabs = useCallback(
    () => applyTabs(() => ({ tabIds: [], activeId: null })),
    [applyTabs],
  );
  const reorderTabs = useCallback((fromId: string, toId: string) => {
    setTabIds((prev) => reorderTabIds(prev, fromId, toId));
  }, []);

  // "+新会话" → swap the detail column to the inline compose form. Open tabs are
  // left untouched (and merely hidden) — composing is a detour, not a close.
  const handleNewSession = () => {
    setPending(null);
    setStartTimedOut(false);
    setComposing(true);
  };

  // Form spawned the process: leave compose mode and start polling for the
  // session so we can swap the column over to its live SessionDetail in place.
  // Snapshot the ad-hoc session ids that exist *now* so the poller can tell the
  // new session apart from the workspace's existing ones (pid can't — see
  // matchSpawnedSession).
  const handleCreated = (info: NewSessionCreated) => {
    setComposing(false);
    setStartTimedOut(false);
    setPending({ ...info, knownIds: new Set(adhocSessions.map((s) => s.id)) });
  };

  // Poll the scanned list for the freshly-spawned session (by id novelty, not
  // pid) and swap the detail column over to it once it appears. `adhocSessions`
  // is filtered to Fleet-owned entrypoints, so this stays scoped to launches.
  useEffect(() => {
    if (!pending) return;
    const match = matchSpawnedSession(adhocSessions, pending);
    if (match) {
      // A freshly spawned session gets its own tab, same as any other open.
      openTab(match, null);
    }
  }, [adhocSessions, pending, openTab]);

  // Surface an escape hatch if the spawn takes unusually long to show up.
  useEffect(() => {
    if (!pending) {
      setStartTimedOut(false);
      return;
    }
    const id = setTimeout(() => setStartTimedOut(true), START_TIMEOUT_MS);
    return () => clearTimeout(id);
  }, [pending]);

  // Dwell-to-read: staying on a session for 2s marks it read. Clicking away (or
  // unmounting) before the timer fires cancels it, so a quick glance doesn't
  // clear the unread dot. Re-keyed on the *active* tab's id, not its activity,
  // so a still-streaming session can flip back to unread and get re-read on a
  // later visit — matching "new message after last read → unread".
  //
  // Following `activeId` (not merely "is open") is what stops background tabs
  // from marking themselves read: a session you have parked in a tab but are
  // not looking at is, correctly, still unread.
  const activeSession = useMemo(
    () => tabs.find((s) => s.id === activeId) ?? null,
    [tabs, activeId],
  );
  // Read at fire time so the timer isn't re-armed by every scan that refreshes
  // the session object, which would keep pushing the 2s dwell out.
  const activeSessionRef = useRef(activeSession);
  activeSessionRef.current = activeSession;
  useEffect(() => {
    if (!activeId || overlaid) return;
    const id = setTimeout(() => {
      const target = activeSessionRef.current;
      if (target) markRead(target);
    }, 2000);
    return () => clearTimeout(id);
  }, [activeId, overlaid, markRead]);

  return (
    <PageShell
      view="history"
      title={t("history.title", "任务")}
      count={rows.length}
      search={{
        value: query,
        onChange: setQuery,
        placeholder: t("history.search_placeholder", "搜索标题、计划、全文…"),
        busy: searching,
      }}
      actions={
        <>
          {/* Icon-only: "Mark all read" spelled out is long enough in en locale to
              crowd the new-session button next to it. */}
          <button
            type="button"
            className={styles.read_btn}
            disabled={unreadSessions.length === 0}
            onClick={() => markManyRead(unreadSessions)}
            title={t("history.mark_all_read_tip", "把所有未读会话标记为已读")}
            aria-label={t("history.mark_all_read", "全部已读")}
          >
            <CheckCheck size={14} strokeWidth={1.8} />
          </button>
          <button
            type="button"
            className={`${styles.new_btn} ${composing ? styles.new_btn_active : ""}`}
            onClick={handleNewSession}
            title={t("new_session.title")}
          >
            <Plus size={12} strokeWidth={2} />
            <span>{t("new_session.button")}</span>
          </button>
        </>
      }
      secondary={
        <>
        <div className={styles.controls}>
          <div className={styles.filter_row}>
            <select
              className={styles.project_select}
              value={workspaceFilter}
              onChange={(e) => setWorkspaceFilter(e.target.value)}
              title={t("history.filter_workspace", "按工作目录筛选")}
            >
              <option value="all">{t("history.all_workspaces", "全部目录")}</option>
              {/* The chat workspace is a category, not a project — pinned above
                  the repos, in both directions. Hidden entirely if the backend
                  couldn't name it, since neither option could then be honoured. */}
              {chatPath && (
                <>
                  <option value={CHAT_ONLY}>{t("history.chat_only", "💬 仅聊天")}</option>
                  <option value={CHAT_HIDDEN}>{t("history.chat_hidden", "🚫 隐藏聊天")}</option>
                </>
              )}
              {workspaces.map(([path, name]) => (
                <option key={path} value={path} title={path}>{name}</option>
              ))}
            </select>
            <button
              type="button"
              className={`${styles.active_toggle} ${activeOnly ? styles.active_toggle_on : ""}`}
              aria-pressed={activeOnly}
              onClick={() => setActiveOnly(!activeOnly)}
              title={t("history.filter_active_tip", "只显示仍在运行或等待输入的会话")}
            >
              <span className={styles.active_toggle_dot} />
              {t("history.only_active", "仅活跃")}
              <span>{activeCount}</span>
            </button>
          </div>
          <div
            className={styles.mark_filter}
            role="group"
            aria-label={t("history.mark_filter_label", "按标记状态筛选")}
          >
            {MARK_SEGMENTS.map((key) => {
              const label =
                key === "all"
                  ? t("history.mark_f_all", "全部")
                  : key === "pending"
                    ? t("history.mark_f_pending", "进行中")
                    : t("history.mark_f_done", "已完成");
              return (
                <button
                  key={key}
                  type="button"
                  className={`${styles.mark_seg} ${markFilter === key ? styles.mark_seg_on : ""}`}
                  data-seg={key}
                  aria-pressed={markFilter === key}
                  onClick={() => setMarkFilter(key)}
                  title={label}
                  aria-label={`${label} (${markCounts[key]})`}
                >
                  {key === "all" && (
                    <span className={styles.mark_seg_label}>{label}</span>
                  )}
                  {key === "pending" && <Circle size={13} strokeWidth={1.8} />}
                  {key === "done" && <CheckCircle2 size={14} strokeWidth={1.8} />}
                  <span className={styles.mark_seg_count}>{markCounts[key]}</span>
                </button>
              );
            })}
          </div>
        </div>

        <div className={styles.list}>
          {!scanReady ? (
            <div className={styles.empty}>{t("scanning")}</div>
          ) : rows.length === 0 ? (
            <div className={styles.empty}>
              {adhocSessions.length === 0
                ? t("history.empty", "还没有会话，点右上角“新会话”发起一个")
                : t("history.no_match", "没有匹配的会话")}
            </div>
          ) : (
            rows.map((s) => (
              <SessionRow
                key={s.jsonlPath}
                session={s}
                snippet={
                  query.trim().length >= 2 ? snippetByPath.get(s.jsonlPath) : undefined
                }
                isSelected={activeId === s.id}
                isOpen={activeId !== s.id && tabIds.includes(s.id)}
                unread={sessionUnread(s, readOverrides[s.id])}
                nowTick={nowTick}
                onClick={handleRowClick}
              />
            ))
          )}
        </div>

        </>
      }
    >
      {/* This column wrapper is load-bearing: `.detail_body` below is a flex ROW
          whose children (the open tabs' panes, the composer, the spawn spinner)
          compete for the same space. Dropping this level would put the tab strip
          side by side with them instead of above them. */}
      <div className={styles.detail}>
        {/* No tab reads as active while the composer or the spawn spinner owns
            the column — the highlighted tab would be pointing at content that
            isn't on screen. */}
        <SessionTabs
          tabs={tabs}
          activeId={overlaid ? null : activeId}
          onActivate={activateTab}
          onClose={closeTab}
          onCloseOthers={closeOtherTabs}
          onCloseRight={closeTabsToRight}
          onCloseAll={closeAllTabs}
          onReorder={reorderTabs}
        />

        <div className={styles.detail_body}>
          {/* Every open tab stays mounted; only the active one is displayed.
              Unmounting the others would throw away the messages, scroll
              position and view-tab that make a tab worth keeping open. The
              hidden ones are `paused`, so they cost no polling. */}
          {tabs.map((tab) => {
            const visible = !overlaid && tab.id === activeId;
            return (
              <div
                key={tab.id}
                className={styles.pane}
                style={{ display: visible ? "flex" : "none" }}
              >
                <SessionDetail
                  inline
                  sessionInfo={tab}
                  searchQuery={queryById[tab.id] ?? null}
                  paused={!visible}
                />
              </div>
            );
          })}

          {pending ? (
            <div className={styles.detail_starting}>
              {startTimedOut ? (
                <>
                  <span className={styles.starting_text}>
                    {t("new_session.start_timeout", "启动较慢，可再等等，或从左侧列表里查看")}
                  </span>
                  <button
                    type="button"
                    className={styles.starting_dismiss}
                    onClick={() => setPending(null)}
                  >
                    {t("cancel")}
                  </button>
                </>
              ) : (
                <>
                  <span className={styles.starting_spinner} />
                  <span className={styles.starting_text}>
                    {t("new_session.starting", "正在启动会话…")}
                  </span>
                </>
              )}
            </div>
          ) : composing ? (
            <NewSessionForm onCreated={handleCreated} onCancel={() => setComposing(false)} />
          ) : tabs.length === 0 ? (
            <div className={styles.detail_empty}>
              <History size={28} strokeWidth={1.2} />
              <span>{t("history.select_hint", "从左侧选择一个会话查看详情")}</span>
            </div>
          ) : null}
        </div>
      </div>
    </PageShell>
  );
}
