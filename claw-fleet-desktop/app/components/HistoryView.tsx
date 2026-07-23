import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { useTranslation } from "react-i18next";
import {
  CheckCheck,
  CheckCircle2,
  Circle,
  Copy,
  Eye,
  Folder,
  FolderOpen,
  History,
  PanelRightOpen,
  Pencil,
  Plus,
  Square,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  useConnectionStore,
  useReadStore,
  useSessionsStore,
  useUIStore,
  type MarkFilter,
} from "../store";
import type { SessionInfo } from "../types";
import { LIVE_STATUSES, isFleetOwnedTask, sessionUnread } from "../types";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { PageShell } from "./PageShell";
import {
  NewSessionForm,
  distinctWorkspaces,
  repoRootPath,
  type NewSessionCreated,
} from "./NewSessionForm";
import { useComposerDraftStore, type ComposerDraft } from "../composerDraft";
import { SessionDetail } from "./SessionDetail";
import { SessionTabs, type TabItem } from "./SessionTabs";
import {
  closeOtherTabs as closeOtherTabsState,
  closeTab as closeTabState,
  closeTabsToRight as closeTabsToRightState,
  DRAFT_TAB_ID,
  openTab as openTabState,
  parsePersistedTabs,
  pruneMissingTabs,
  replaceTab as replaceTabState,
  reorderTabs as reorderTabIds,
  TABS_STORAGE_KEY,
  type TabState,
} from "../sessionTabs";
import { getItem, setItem } from "../storage";
import { canControl, stopMode, performStop } from "./StopControl";
import { SessionRail } from "./SessionRail";
import { ContextMenu, type ContextMenuItem, type ContextMenuAnchor } from "./ContextMenu";
import { RenameSessionDialog } from "./RenameSessionDialog";
import { buildRenderItems, dwellReadTargets } from "./sessionGroups";
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
 * Keep inactive session panes mounted *and laid out*. On macOS, WKWebView can
 * restore an overflow scroller from `display: none` without repainting its
 * contents; the first physical scroll then makes the conversation reappear.
 * An invisible absolute pane keeps its own box and scroll layer alive without
 * competing with the active pane in the flex row or accepting input.
 */
export function sessionPaneStyle(visible: boolean): CSSProperties {
  return visible
    ? {
        visibility: "visible",
        position: "relative",
        pointerEvents: "auto",
      }
    : {
        visibility: "hidden",
        position: "absolute",
        inset: 0,
        pointerEvents: "none",
      };
}

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

// timeAgo / formatRunning / renderSnippet / sessionEq and the SessionRow
// component itself moved to ./SessionRow (the shared row atom), so LiteApp's
// rail renders the identical row without duplicating it here.

/**
 * Reorder `rows` (the live filtered+sorted list) back into a frozen id order,
 * used to hold the list still while the pointer is parked over it. Rows named in
 * `frozenOrder` keep that order (their live content is preserved — we resolve
 * each id against `rows`); rows absent from the snapshot — freshly spawned, or
 * newly matching the filter — are appended in their natural sort position rather
 * than spliced into the frozen block. Ids in `frozenOrder` whose row has since
 * dropped out of `rows` simply fall away. `null` = no freeze, pass through.
 */
export function applyFrozenOrder(
  rows: SessionInfo[],
  frozenOrder: string[] | null,
): SessionInfo[] {
  if (!frozenOrder) return rows;
  const byId = new Map(rows.map((s) => [s.id, s]));
  const seen = new Set<string>();
  const ordered: SessionInfo[] = [];
  for (const id of frozenOrder) {
    const s = byId.get(id);
    if (s) {
      ordered.push(s);
      seen.add(id);
    }
  }
  for (const s of rows) if (!seen.has(s.id)) ordered.push(s);
  return ordered;
}

// Relay-chain grouping logic (RenderItem / buildRenderItems / chainBarColor /
// dwellReadTargets / GROUP_VISIBLE / GROUP_LOAD_STEP / chainTip) lives in
// ./sessionGroups so LiteApp's rail can reuse it without importing this file.

// GroupMarkControl (relay-chain mark-all) moved to ./MarkControl alongside the
// single-row MarkControl; SessionRail renders it inside group headers.


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
  // Remote workspaces live on the probe host — their files can't be revealed in
  // the local file manager, so the row menu hides that item for them.
  const connection = useConnectionStore((s) => s.connection);
  const isLocal = connection?.type !== "remote";

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
  // Collapse handoff-relay chains into one row (default on; togglable in
  // Settings). Read from the store so the choice survives HistoryView's frequent
  // unmounts and reacts live when flipped in the settings panel.
  const groupHandoff = useUIStore((s) => s.historyGroupHandoff);
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
  // The "+新会话" flow lives entirely inside a synthetic draft tab (DRAFT_TAB_ID)
  // rather than as a column-level overlay. That tab renders the compose form;
  // once the form spawns a session, `pending` flips its pane to a "starting…"
  // spinner, and the morph effect below swaps DRAFT_TAB_ID for the real session
  // id in place — the draft tab becomes the session's SessionDetail without
  // leaving its slot. `pending`/`startTimedOut` are therefore about the draft
  // tab's pane content, not a separate mode covering the whole column.
  const [pending, setPending] = useState<PendingSpawn | null>(null);
  const [startTimedOut, setStartTimedOut] = useState(false);

  // Sort freeze: while the pointer is over the list, hold the row *order* still
  // so a scan landing under the cursor can't re-sort a row out from under a
  // click. `frozenOrder` is the session-id order captured on enter; null means
  // live sorting. New sessions that appear while frozen are appended at the end
  // (see `displayRows`), never spliced into the middle. The order alone is
  // frozen — each row's *content* still updates from the live scan, and filters
  // still apply. Cleared on leave and on window blur (pointer yanked out of the
  // window without a mouseleave, same failure the per-row hover guards handle).
  const [frozenOrder, setFrozenOrder] = useState<string[] | null>(null);

  const { searching, ftsMatchPaths, snippetByPath } = useSessionSearch(query);

  const adhocSessions = useMemo(
    () => sessions.filter(isFleetOwnedTask),
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
          (s.titleOverride?.toLowerCase().includes(q) ?? false) ||
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

  // Whether the task page currently mixes agent sources (Claude + Codex + …).
  // Only then does the per-row source glyph earn its place; a uniform list gets
  // no badge. Scoped to the launchpad's own sessions, not the global scan, so
  // the badge tracks what this page actually shows.
  const multiSource = useMemo(
    () => new Set(adhocSessions.map((s) => s.agentSource)).size > 1,
    [adhocSessions],
  );

  // The order shown, honouring the sort freeze. `rows` stays the live
  // filtered+sorted list (its length still drives the header count); this only
  // reshuffles it back into the frozen order while the pointer is parked.
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const displayRows = useMemo(
    () => applyFrozenOrder(rows, frozenOrder),
    [rows, frozenOrder],
  );

  // Fold the flat display list into groups + singles. A no-op (all singles)
  // when grouping is off, so the render loop below is uniform either way.
  const displayItems = useMemo(
    () => buildRenderItems(displayRows, groupHandoff),
    [displayRows, groupHandoff],
  );

  // Full membership of every relay chain, keyed by chainId — taken over ALL
  // launchpad sessions (not the filtered rows) so a group's mark-all covers the
  // whole chain even when the mark filter is hiding some hops, and the header's
  // aggregate done-state reflects the entire chain.
  const chainMembersAll = useMemo(() => {
    const m = new Map<string, SessionInfo[]>();
    for (const s of adhocSessions) {
      if (s.handoff && s.handoff.chainLen > 1) {
        const arr = m.get(s.handoff.chainId);
        if (arr) arr.push(s);
        else m.set(s.handoff.chainId, [s]);
      }
    }
    return m;
  }, [adhocSessions]);

  // Tip id → full chain membership, but only for chains currently rendered as a
  // collapsed group header. Backs dwell-read (see `dwellReadTargets`): opening a
  // group header must clear the whole chain's aggregate unread dot, not just the
  // tip — which is all the header click actually opens. Singles and expanded
  // children are absent from the map, so they fall back to marking only
  // themselves.
  const groupHeaderChains = useMemo(() => {
    const m = new Map<string, SessionInfo[]>();
    for (const it of displayItems) {
      if (it.kind === "group") {
        m.set(it.tip.id, chainMembersAll.get(it.chainId) ?? it.members);
      }
    }
    return m;
  }, [displayItems, chainMembersAll]);

  // Chain expand / page-in state now lives inside <SessionRail>.

  const freezeSort = useCallback(() => {
    setFrozenOrder((prev) => prev ?? rowsRef.current.map((s) => s.id));
  }, []);
  const thawSort = useCallback(() => setFrozenOrder(null), []);
  // A pointer yanked out of the window (drag onto another app, alt-tab) fires no
  // mouseleave; clear the freeze on blur so it can't latch on forever.
  useEffect(() => {
    if (!frozenOrder) return;
    window.addEventListener("blur", thawSort);
    return () => window.removeEventListener("blur", thawSort);
  }, [frozenOrder, thawSort]);

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
  // The strip's entries. The draft tab (DRAFT_TAB_ID) has no backing session and
  // renders the compose form / starting spinner; every other id resolves to its
  // live session, and an id that resolves to nothing (a vanished session) drops
  // out just as before.
  const tabItems = useMemo<TabItem[]>(
    () =>
      tabIds
        .map((id): TabItem | null => {
          if (id === DRAFT_TAB_ID)
            return { id, session: null, label: t("new_session.button") };
          const s = sessionById.get(id);
          return s ? { id, session: s } : null;
        })
        .filter((x): x is TabItem => x != null),
    [tabIds, sessionById, t],
  );

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
    // The draft tab never resolves against the scan — exempt it from pruning, or
    // the first scan would close a new-session form the user is typing into.
    const next = pruneMissingTabs(
      { tabIds, activeId },
      (id) => id === DRAFT_TAB_ID || sessionById.has(id),
    );
    if (next.tabIds.length === tabIds.length) return;
    setTabIds(next.tabIds);
    setActiveId(next.activeId);
  }, [scanReady, sessionById, tabIds, activeId]);

  // Switching tabs just moves focus. A spawn in flight keeps correlating in the
  // background (its draft tab morphs in place whether or not it's on screen), so
  // activating another tab must NOT cancel `pending`.
  const activateTab = useCallback((id: string) => setActiveId(id), []);

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

  // Row context menu. Anchor + subject are held here (not via useContextMenu) so
  // the open handler stays referentially stable — SessionRow is memoised and a
  // fresh onContextMenu each render would re-render every row on every scan.
  const [menuAnchor, setMenuAnchor] = useState<ContextMenuAnchor | null>(null);
  const [menuSession, setMenuSession] = useState<SessionInfo | null>(null);
  const openRowMenu = useCallback((e: React.MouseEvent, s: SessionInfo) => {
    // Suppress the app-wide (Settings/About/Quit) menu, same as ContextMenu's
    // own callers do.
    e.preventDefault();
    e.stopPropagation();
    setMenuSession(s);
    setMenuAnchor({ x: e.clientX, y: e.clientY });
  }, []);
  const closeRowMenu = useCallback(() => {
    setMenuAnchor(null);
    setMenuSession(null);
  }, []);

  // Manual title override ("重命名"). Held here so the dialog outlives the
  // context menu that opened it (the menu closes on select).
  const [renameTarget, setRenameTarget] = useState<SessionInfo | null>(null);

  // A failed clipboard write (Tauri ACL) must not read as success. The menu is
  // gone by the time the promise settles, so failures are swallowed rather than
  // faked — the same fire-and-forget the header menu uses for reveal.
  const copyText = useCallback((text: string) => {
    writeText(text).catch(() => {});
  }, []);

  const rowMenuItems = useCallback(
    (s: SessionInfo): ContextMenuItem[] => {
      const unread = sessionUnread(s, readOverrides[s.id]);
      const isDone = s.userMark === "done";
      const revealKey =
        document.documentElement.getAttribute("data-platform") === "windows"
          ? "paths.reveal_in_explorer"
          : "paths.reveal_in_finder";
      const items: ContextMenuItem[] = [
        {
          id: "open",
          label: t("history.menu_open", "打开"),
          icon: <PanelRightOpen size={13} />,
          onSelect: () => handleRowClick(s),
        },
      ];
      if (unread) {
        items.push({
          id: "mark-read",
          label: t("history.menu_mark_read", "标为已读"),
          icon: <Eye size={13} />,
          onSelect: () => markRead(s),
        });
      }
      items.push({
        id: "toggle-mark",
        label: isDone
          ? t("history.menu_mark_pending", "标为进行中")
          : t("history.menu_mark_done", "标为已完成"),
        icon: isDone ? <Circle size={13} /> : <CheckCircle2 size={13} />,
        onSelect: () => {
          // Same backend call the row's MarkControl fires; the store converges
          // via the `sessions-updated` it emits.
          invoke("set_session_mark", {
            sessionId: s.id,
            workspacePath: s.workspacePath,
            mark: isDone ? null : "done",
          }).catch(() => {});
        },
      });
      items.push({
        id: "rename",
        label: t("history.menu_rename", "重命名"),
        icon: <Pencil size={13} />,
        onSelect: () => setRenameTarget(s),
      });
      items.push({
        id: "copy-id",
        label: t("detail.copy_session_id", "复制会话 ID"),
        sub: s.id,
        icon: <Copy size={13} />,
        onSelect: () => copyText(s.id),
      });
      items.push({
        id: "copy-workspace",
        label: t("detail.copy_workspace_path", "复制工作目录"),
        sub: s.workspacePath,
        icon: <Folder size={13} />,
        onSelect: () => copyText(s.workspacePath),
      });
      if (isLocal) {
        items.push({
          id: "reveal",
          label: t(revealKey),
          icon: <FolderOpen size={13} />,
          onSelect: () => {
            invoke("reveal_path", { path: s.workspacePath }).catch(() => {});
          },
        });
      }
      if (canControl(s) && stopMode(s) !== "spent") {
        items.push({
          id: "stop",
          label: t("history.menu_stop", "停止会话"),
          icon: <Square size={13} />,
          danger: true,
          onSelect: () => {
            void performStop(s, t);
          },
        });
      }
      return items;
    },
    [t, readOverrides, isLocal, handleRowClick, markRead, copyText],
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

  // Back out of the draft tab: close it and abandon any in-flight spawn
  // correlation so a late scan match doesn't pop the tab back open.
  const cancelDraft = useCallback(() => {
    setPending(null);
    setStartTimedOut(false);
    applyTabs((s) => closeTabState(s, DRAFT_TAB_ID));
  }, [applyTabs]);

  // "+新会话" → open (or refocus) the single draft tab. Other tabs are left
  // untouched; the draft is just another tab, so this is `openTab`, not an
  // overlay. Clicking it again while a draft is already open simply refocuses it.
  const handleNewSession = () => {
    applyTabs((st) => openTabState(st, DRAFT_TAB_ID));
  };

  // Schedule page "新建" shortcut: seed the new-session composer with a
  // scheduling-assistant template, then open (or refocus) the draft tab. The
  // store hop to list/gallery mounts this view; we react to the nonce once.
  const newSessionNav = useUIStore((s) => s.newSessionNav);
  const clearNewSessionNav = useUIStore((s) => s.clearNewSessionNav);
  const handledNavNonce = useRef<number | null>(null);
  useEffect(() => {
    if (!newSessionNav) return;
    if (handledNavNonce.current === newSessionNav.nonce) return;
    handledNavNonce.current = newSessionNav.nonce;
    // Seed the "new" draft with every field the request carried. The schedule
    // page's "立即运行" fills workspace/model/effort/tool so the draft opens as
    // a ready-to-send copy of the task; the "新建" shortcut passes only prompt.
    const seed: Partial<ComposerDraft> = {};
    if (newSessionNav.prompt) seed.prompt = newSessionNav.prompt;
    if (newSessionNav.workspace) seed.workspace = newSessionNav.workspace;
    if (newSessionNav.model !== undefined) seed.model = newSessionNav.model;
    if (newSessionNav.effort !== undefined) seed.effort = newSessionNav.effort;
    if (newSessionNav.tool) seed.tool = newSessionNav.tool;
    if (newSessionNav.permissionMode) seed.permissionMode = newSessionNav.permissionMode;
    if (Object.keys(seed).length > 0) {
      useComposerDraftStore.getState().patchDraft("new", seed);
    }
    applyTabs((st) => openTabState(st, DRAFT_TAB_ID));
    clearNewSessionNav();
  }, [newSessionNav, applyTabs, clearNewSessionNav]);

  // A notification / tray click on a Fleet-spawned session routes here (the
  // store hop to "history" mounts this view); open the session in the inline
  // tab strip. Tabs are kept as ids and resolved against the live scan, so we
  // only need the id — the sender already gated on isFleetOwnedTask, i.e. the
  // session is one of ours and appears in adhocSessions. React to the nonce once.
  const openTaskNav = useUIStore((s) => s.openTaskNav);
  const clearOpenTaskNav = useUIStore((s) => s.clearOpenTaskNav);
  const handledOpenTaskNonce = useRef<number | null>(null);
  useEffect(() => {
    if (!openTaskNav) return;
    if (handledOpenTaskNonce.current === openTaskNav.nonce) return;
    handledOpenTaskNonce.current = openTaskNav.nonce;
    applyTabs((st) => openTabState(st, openTaskNav.sessionId));
    clearOpenTaskNav();
  }, [openTaskNav, applyTabs, clearOpenTaskNav]);

  // Form spawned the process: flip the draft tab's pane to the "starting…"
  // spinner and start polling for the session. Snapshot the ad-hoc session ids
  // that exist *now* so the poller can tell the new session apart from the
  // workspace's existing ones (pid can't — see matchSpawnedSession).
  const handleCreated = (info: NewSessionCreated) => {
    setStartTimedOut(false);
    setPending({ ...info, knownIds: new Set(adhocSessions.map((s) => s.id)) });
  };

  // Poll the scanned list for the freshly-spawned session (by id, not pid) and
  // morph the draft tab into it in place once it appears. If the user closed the
  // draft tab mid-spawn, abandon the correlation rather than reopening a tab
  // behind their back. `adhocSessions` is Fleet-owned only, so this stays scoped
  // to launches.
  useEffect(() => {
    if (!pending) return;
    if (!tabIds.includes(DRAFT_TAB_ID)) {
      setPending(null);
      setStartTimedOut(false);
      return;
    }
    const match = matchSpawnedSession(adhocSessions, pending);
    if (match) {
      applyTabs((st) => replaceTabState(st, DRAFT_TAB_ID, match.id));
      setQueryById((prev) => ({ ...prev, [match.id]: null }));
      setPending(null);
      setStartTimedOut(false);
    }
  }, [adhocSessions, pending, tabIds, applyTabs]);

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
    () => tabItems.find((tab) => tab.id === activeId)?.session ?? null,
    [tabItems, activeId],
  );
  // Read at fire time so the timer isn't re-armed by every scan that refreshes
  // the session object, which would keep pushing the 2s dwell out. Same reason
  // `groupHeaderChains` is read through a ref: it's rebuilt on every scan, so a
  // dep on it would reset the dwell timer each tick.
  const activeSessionRef = useRef(activeSession);
  activeSessionRef.current = activeSession;
  const groupHeaderChainsRef = useRef(groupHeaderChains);
  groupHeaderChainsRef.current = groupHeaderChains;
  useEffect(() => {
    // The draft tab has no session to mark read; only real session tabs dwell.
    if (!activeId || activeId === DRAFT_TAB_ID) return;
    const id = setTimeout(() => {
      const target = activeSessionRef.current;
      // A group header aggregates unread over the whole chain, so dwelling on it
      // clears every member — not just the tip the click opened.
      if (target) markManyRead(dwellReadTargets(target, groupHeaderChainsRef.current));
    }, 2000);
    return () => clearTimeout(id);
  }, [activeId, markManyRead]);

  // One session row — shared by standalone rows and the members inside an
  // expanded handoff group, so both stay pixel-identical and pick up the same
  // memoisation.
  // Fold this page's query threshold, read overrides and open-tab set into the
  // shape <SessionRail> takes, so the shared rail stays agnostic of the stores.
  const railSnippetFor = (jsonlPath: string) =>
    query.trim().length >= 2 ? snippetByPath.get(jsonlPath) : undefined;
  const railIsUnread = (s: SessionInfo) => sessionUnread(s, readOverrides[s.id]);
  const openTabIds = useMemo(() => new Set(tabIds), [tabIds]);

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
            className={`${styles.new_btn} ${activeId === DRAFT_TAB_ID ? styles.new_btn_active : ""}`}
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
          </div>
          {/* Row 2: the "only active" pill sits beside the mark segments rather
              than inside the workspace-select row. On WebKit (Tauri's WKWebView)
              a <select> refuses to shrink below its widest option even with
              min-width:0, so pairing it with the pill wrapped the pill onto its
              own orphaned line. Giving the pill its own row removes that
              dependency and reads the same in both engines. */}
          <div className={styles.mark_row}>
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
        </div>

        <div
          className={styles.list}
          onMouseEnter={freezeSort}
          onMouseLeave={thawSort}
        >
          {!scanReady ? (
            <div className={styles.empty}>{t("scanning")}</div>
          ) : rows.length === 0 ? (
            <div className={styles.empty}>
              {adhocSessions.length === 0
                ? t("history.empty", "还没有会话，点右上角“新会话”发起一个")
                : t("history.no_match", "没有匹配的会话")}
            </div>
          ) : (
            <SessionRail
              items={displayItems}
              chainMembersAll={chainMembersAll}
              activeId={activeId}
              openIds={openTabIds}
              snippetFor={railSnippetFor}
              isUnread={railIsUnread}
              nowTick={nowTick}
              showSource={multiSource}
              onRowClick={handleRowClick}
              onContextMenu={openRowMenu}
            />
          )}
          {menuAnchor && menuSession && (
            <ContextMenu
              anchor={menuAnchor}
              items={rowMenuItems(menuSession)}
              onClose={closeRowMenu}
            />
          )}
          {renameTarget && (
            <RenameSessionDialog
              currentTitle={
                renameTarget.titleOverride ??
                renameTarget.aiTitle ??
                renameTarget.slug ??
                ""
              }
              hasOverride={renameTarget.titleOverride != null}
              onCancel={() => setRenameTarget(null)}
              onSave={(title) => {
                const target = renameTarget;
                setRenameTarget(null);
                invoke("set_session_title", {
                  sessionId: target.id,
                  workspacePath: target.workspacePath,
                  title: title === "" ? null : title,
                }).catch(() => {});
              }}
            />
          )}
        </div>

        </>
      }
    >
      {/* This column wrapper is load-bearing: `.detail_body` below is a flex ROW
          whose children (the open tabs' panes) compete for the same space.
          Dropping this level would put the tab strip side by side with them
          instead of above them. */}
      <div className={styles.detail}>
        <SessionTabs
          tabs={tabItems}
          activeId={activeId}
          onActivate={activateTab}
          onClose={closeTab}
          onCloseOthers={closeOtherTabs}
          onCloseRight={closeTabsToRight}
          onCloseAll={closeAllTabs}
          onReorder={reorderTabs}
        />

        <div className={styles.detail_body}>
          {/* Every open tab stays mounted; only the active one is visible.
              Unmounting the others would throw away the messages, scroll
              position and view-tab that make a tab worth keeping open. The
              hidden ones are `paused`, so they cost no polling. The draft tab
              renders the compose form, or the spawn spinner once it has fired. */}
          {tabItems.map((tab) => {
            const visible = tab.id === activeId;
            return (
              <div
                key={tab.id}
                className={styles.pane}
                style={sessionPaneStyle(visible)}
                aria-hidden={!visible}
              >
                {tab.id === DRAFT_TAB_ID ? (
                  pending ? (
                    <div className={styles.detail_starting}>
                      {startTimedOut ? (
                        <>
                          <span className={styles.starting_text}>
                            {t("new_session.start_timeout", "启动较慢，可再等等，或从左侧列表里查看")}
                          </span>
                          <button
                            type="button"
                            className={styles.starting_dismiss}
                            onClick={cancelDraft}
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
                  ) : (
                    <NewSessionForm onCreated={handleCreated} onCancel={cancelDraft} />
                  )
                ) : (
                  <SessionDetail
                    inline
                    sessionInfo={tab.session!}
                    searchQuery={queryById[tab.id] ?? null}
                    paused={!visible}
                  />
                )}
              </div>
            );
          })}

          {tabItems.length === 0 && (
            <div className={styles.detail_empty}>
              <History size={28} strokeWidth={1.2} />
              <span>{t("history.select_hint", "从左侧选择一个会话查看详情")}</span>
            </div>
          )}
        </div>
      </div>
    </PageShell>
  );
}
