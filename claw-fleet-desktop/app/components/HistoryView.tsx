import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  CheckCircle2,
  Circle,
  Copy,
  Folder,
  FolderOpen,
  PanelRightOpen,
  Pencil,
  Plus,
  Square,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  useSessionsStore,
  useUIStore,
  type MarkFilter,
} from "../store";
import type { SessionInfo } from "../types";
import { isFleetOwnedTask } from "../types";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { PageShell } from "./PageShell";
import { NewSessionForm, type NewSessionCreated } from "./NewSessionForm";
import { useComposerDraftStore, type ComposerDraft } from "../composerDraft";
import { SessionDetail } from "./SessionDetail";
import { getItem, setItem } from "../storage";
import { canControl, stopMode, performStop } from "./StopControl";
import { SessionRail, WorkspaceRailSection } from "./SessionRail";
import { ContextMenu, type ContextMenuItem, type ContextMenuAnchor } from "./ContextMenu";
import { RenameSessionDialog } from "./RenameSessionDialog";
import { buildRenderItems } from "./sessionGroups";
import { groupSessionsByWorkspace } from "./workspaceSessionGroups";
import styles from "./HistoryView.module.css";
import { canRevealPath } from "../canReveal";

/** A session spawned but not yet discovered by the scanner. We poll the session
 *  list for the matching `SessionInfo` and swap the detail column over to it. */
export interface PendingSpawn extends NewSessionCreated {
  /** Ad-hoc session ids that already existed when the spawn returned. The new
   *  session is the one whose id is NOT in this set — pid can't identify it
   *  (see `matchSpawnedSession`). */
  knownIds: Set<string>;
}

/** Give up on the in-place spinner after this long and offer an escape hatch;
 *  the scanner polls every ~5s so a healthy spawn resolves well within this. */
const START_TIMEOUT_MS = 30_000;

/** Sentinel `openId` for the new-session composer. A real session id is a UUID,
 *  so it can never collide with this. */
const DRAFT_ID = "new:draft";

/** Which session the detail column was showing when the app last closed. */
const OPEN_PANE_STORAGE_KEY = "launchpad-open";


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
export function matchSpawnedSession(
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
 *  map to the binary mark buckets (unmarked collapses to "pending").
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

// timeAgo / formatRunning / renderSnippet / sessionEq and the SessionRow
// component itself moved to ./SessionRow (the shared row atom) so every rail
// renders the identical row without duplicating it here.

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
// GROUP_VISIBLE / GROUP_LOAD_STEP / chainTip) lives in
// ./sessionGroups so any rail can reuse it without importing this file.

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

  // Rail filters live in the store, not here: this component is unmounted every
  // time `viewMode` leaves "history", which would otherwise reset them behind
  // the user's back (a waiting-input alert forcing setViewMode("list") is enough).
  const query = useUIStore((s) => s.historyQuery);
  const setQuery = useUIStore((s) => s.setHistoryQuery);
  // Re-render on a slow tick so the relative "last updated" and the live
  // Elapsed-runtime durations keep counting even when no scan lands (a waiting-input
  // session can sit idle for minutes). 30s granularity matches the minute-level
  // display without churning the list.
  const [nowTick, setNowTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setNowTick((n) => n + 1), 30_000);
    return () => clearInterval(id);
  }, []);
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
  // What the detail column holds: one session id, the new-session draft, or
  // nothing. The column used to be an IDE-style split of tabbed editor groups;
  // it is a single pane now, so this is the whole layout state. We keep an
  // *id*, not a SessionInfo snapshot, and resolve it against the live scan on
  // every render — a session open for ten minutes must show its current title
  // and status, not the ones it wore when it was opened.
  //
  // Restored from the previous run. The lazy initialiser matters: `initStorage`
  // is awaited before React renders, so reading at first render sees a warm
  // cache, while a module-level read would run at import time and see nothing.
  const [openId, setOpenId] = useState<string | null>(
    () => getItem(OPEN_PANE_STORAGE_KEY) || null,
  );
  // Search highlight (the FTS query that matched that session), keyed by
  // session id — each session was opened by its own click and carries its own.
  const [queryById, setQueryById] = useState<Record<string, string | null>>({});
  // The "+新会话" flow is the draft the column holds (`openId === DRAFT_ID`):
  // the pane renders the compose form, and once the form spawns a session,
  // `pending` flips it to a "starting…" spinner until the scan surfaces the
  // session and the column switches to it.
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

  // The pure-chat workspace. It is no longer a mode of its own — chat sessions
  // sit in the rail like any other section — but the section is pinned to the
  // top (see `pinnedPath` below) so it never sinks among busier repos.
  const chatPath = useChatWorkspace();

  const { rows, markCounts } = useMemo(() => {
    const q = query.trim().toLowerCase();
    // Everything except the mark filter — the segment counts are taken over
    // this set so each count reflects how many rows its segment would reveal
    // under the current query.
    const preMark = adhocSessions
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
  }, [adhocSessions, query, ftsMatchPaths, markFilter]);

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

  // Repository sections are the rail's primary hierarchy. Relay chains stay a
  // secondary grouping inside each repository instead of joining sessions from
  // separate directories into one flat stream.
  // While the sort freeze holds, the grouping must take `displayRows` as-is —
  // it re-sorts by activity otherwise, which would put the row back under the
  // cursor's feet even though `displayRows` itself was frozen.
  const workspaceGroups = useMemo(
    () =>
      groupSessionsByWorkspace(displayRows, {
        preserveOrder: frozenOrder != null,
        pinnedPath: chatPath,
      }).map((group) => ({
        ...group,
        items: buildRenderItems(group.sessions, groupHandoff),
      })),
    [displayRows, groupHandoff, frozenOrder, chatPath],
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

  // Open tabs, resolved against the live scan. An id whose session has vanished
  // from the scan resolves to nothing and simply drops out of the strip; we
  // deliberately do NOT prune `tabIds` for it, so a session that blips out of a
  // scan cycle comes back to its tab rather than being silently closed.
  const sessionById = useMemo(
    () => new Map(sessions.map((s) => [s.id, s])),
    [sessions],
  );
  // Persist the open pane so it comes back on the next launch. The search
  // highlight is deliberately NOT persisted — a term you searched for last week
  // has no business highlighting a transcript on a cold start.
  useEffect(() => {
    setItem(OPEN_PANE_STORAGE_KEY, openId ?? "");
  }, [openId]);

  // Drop search highlights for sessions no longer open. Returns `prev`
  // unchanged when nothing was dropped, or this would loop.
  useEffect(() => {
    setQueryById((prev) => {
      const kept = Object.entries(prev).filter(([id]) => id === openId);
      return kept.length === Object.keys(prev).length ? prev : Object.fromEntries(kept);
    });
  }, [openId]);

  // A restored id can name a session that has since been deleted; it renders as
  // nothing, so drop it. Once, against the first scan that lands — before it,
  // `sessions` is empty and this would close everything.
  const prunedRef = useRef(false);
  useEffect(() => {
    if (!scanReady || prunedRef.current) return;
    prunedRef.current = true;
    setOpenId((prev) =>
      prev == null || prev === DRAFT_ID || sessionById.has(prev) ? prev : null,
    );
  }, [scanReady, sessionById]);

  const openTab = useCallback((s: SessionInfo, highlight: string | null) => {
    setOpenId(s.id);
    setQueryById({ [s.id]: highlight });
  }, []);

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
      if (canRevealPath()) {
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
    [t, handleRowClick, copyText],
  );

  // Back out of the composer, abandoning any in-flight spawn correlation so a
  // late scan match doesn't pull the column back onto it.
  const cancelDraft = useCallback(() => {
    setPending(null);
    setStartTimedOut(false);
    setOpenId(null);
  }, []);

  // "+新会话" → put the composer in the pane.
  const handleNewSession = () => {
    setOpenId(DRAFT_ID);
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
    setOpenId(DRAFT_ID);
    clearNewSessionNav();
  }, [newSessionNav, clearNewSessionNav]);

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
    setOpenId(openTaskNav.sessionId);
    clearOpenTaskNav();
  }, [openTaskNav, clearOpenTaskNav]);

  // Form spawned the process: flip the draft tab's pane to the "starting…"
  // spinner and start polling for the session. Snapshot the ad-hoc session ids
  // that exist *now* so the poller can tell the new session apart from the
  // workspace's existing ones (pid can't — see matchSpawnedSession).
  const handleCreated = (info: NewSessionCreated) => {
    setStartTimedOut(false);
    setPending({ ...info, knownIds: new Set(adhocSessions.map((s) => s.id)) });
  };

  // Poll the scanned list for the freshly-spawned session (by id, not pid) and
  // switch the pane to it once it appears. If the user navigated away from the
  // composer mid-spawn, abandon the correlation rather than yanking the column
  // back. `adhocSessions` is Fleet-owned only, so this stays scoped to launches.
  useEffect(() => {
    if (!pending) return;
    if (openId !== DRAFT_ID) {
      setPending(null);
      setStartTimedOut(false);
      return;
    }
    const match = matchSpawnedSession(adhocSessions, pending);
    if (match) {
      setOpenId(match.id);
      setQueryById({ [match.id]: null });
      setPending(null);
      setStartTimedOut(false);
    }
  }, [adhocSessions, pending, openId]);

  // Surface an escape hatch if the spawn takes unusually long to show up.
  useEffect(() => {
    if (!pending) {
      setStartTimedOut(false);
      return;
    }
    const id = setTimeout(() => setStartTimedOut(true), START_TIMEOUT_MS);
    return () => clearTimeout(id);
  }, [pending]);

  const activeSession = useMemo(
    () => (openId == null || openId === DRAFT_ID ? null : sessionById.get(openId) ?? null),
    [openId, sessionById],
  );

  // One session row — shared by standalone rows and the members inside an
  // expanded handoff group, so both stay pixel-identical and pick up the same
  // memoisation.
  // Fold this page's query threshold and open-tab set into the shape
  // <SessionRail> takes, so the shared rail stays agnostic of the stores.
  const railSnippetFor = (jsonlPath: string) =>
    query.trim().length >= 2 ? snippetByPath.get(jsonlPath) : undefined;
  // The rail highlights the one session the column is showing.
  const openTabIds = useMemo(
    () => new Set(activeSession ? [activeSession.id] : []),
    [activeSession],
  );
  const railActiveId = activeSession?.id ?? null;

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
      secondary={
        <>
        <div className={styles.rail_launch}>
          <button
            type="button"
            data-wizard="new-session-btn"
            className={`${styles.new_btn} ${openId === DRAFT_ID ? styles.new_btn_active : ""}`}
            onClick={handleNewSession}
            title={t("new_session.title")}
          >
            <Plus size={15} strokeWidth={2} />
            <span>{t("new_session.button")}</span>
          </button>
        </div>
        <div className={styles.controls}>
          <div className={styles.mark_row}>
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
                ? t("history.empty", "还没有会话，点上方“新会话”发起一个")
                : t("history.no_match", "没有匹配的会话")}
            </div>
          ) : (
            workspaceGroups.map((workspace) => (
              <WorkspaceRailSection
                key={workspace.path}
                path={workspace.path}
                name={workspace.name}
                // 折叠后的行数：一条接力链折成一组只算 1，与眼下看到的行一致。
                count={workspace.items.length}
              >
                <SessionRail
                  items={workspace.items}
                  chainMembersAll={chainMembersAll}
                  activeId={railActiveId}
                  openIds={openTabIds}
                  snippetFor={railSnippetFor}
                  nowTick={nowTick}
                  showSource={multiSource}
                  showWorkspace={false}
                  onRowClick={handleRowClick}
                  onContextMenu={openRowMenu}
                />
              </WorkspaceRailSection>
            ))
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
      {/* The detail column: one pane. It holds the session you picked in the
          rail, the new-session composer, or — with nothing picked — the
          composer as its resting state, because the one thing you can do from
          an empty column is start work. */}
      <div className={styles.detail}>
        <div className={styles.detail_body}>
          {activeSession ? (
            <div className={styles.pane}>
              <SessionDetail
                inline
                sessionInfo={activeSession}
                searchQuery={queryById[activeSession.id] ?? null}
              />
            </div>
          ) : openId === DRAFT_ID ? (
            <div className={styles.pane}>
              {pending ? (
                <div className={styles.detail_starting}>
                  {startTimedOut ? (
                    <>
                      <span className={styles.starting_text}>
                        {t(
                          "new_session.start_timeout",
                          "启动较慢，可再等等，或从左侧列表里查看",
                        )}
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
              )}
            </div>
          ) : (
            <div className={styles.pane}>
              <NewSessionForm
                // No `onCancel`: this form is the pane's resting state, not
                // something opened on top of anything, so there is nothing to
                // close back to (the × is hidden with it).
                onCreated={(info) => {
                  // Put the column on the draft *before* arming the
                  // correlation — the spawn poller gives up unless it is there.
                  setOpenId(DRAFT_ID);
                  handleCreated(info);
                }}
              />
            </div>
          )}
        </div>
      </div>
    </PageShell>
  );
}
