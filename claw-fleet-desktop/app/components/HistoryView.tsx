import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  Copy,
  Folder,
  FolderOpen,
  PanelRightOpen,
  Pencil,
  Plus,
  Square,
  X,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useDecisionStore, useSessionsStore, useUIStore } from "../store";
import type { PendingDecision, SessionInfo } from "../types";
import { isFleetOwnedTask, isQuietAliveSticky } from "../types";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { PageShell } from "./PageShell";
import {
  NewSessionForm,
  type NewSessionCreated,
  type NewSessionLaunch,
} from "./NewSessionForm";
import { ConversationSkeleton } from "./MessageList";
import { SkeletonList, Spinner } from "./loading";
import { useComposerDraftStore, type ComposerDraft } from "../composerDraft";
import { SessionDetail } from "./SessionDetail";
import { DecisionDock } from "./DecisionDock";
import { getItem, setItem } from "../storage";
import { canControl, stopMode, performStop } from "./StopControl";
import { ContextMenu, type ContextMenuItem, type ContextMenuAnchor } from "./ContextMenu";
import { RenameSessionDialog } from "./RenameSessionDialog";
import {
  buildHomeRows,
  filterRows,
  firstNeedsYou,
  nextNeedsYou,
  rowDecisions,
  rowOfSession,
  rowSessionId,
  sectionRows,
  type HomeRow,
  type TaskRow,
} from "./homeRows";
import { TaskList, rowMenuSession } from "./TaskList";
import styles from "./HistoryView.module.css";
import { canRevealPath } from "../canReveal";
import { Presence } from "./Presence";
import { PillMenu } from "./PillMenu";
import pillStyles from "./PillMenu.module.css";

/** A session spawned but not yet discovered by the scanner. We poll the session
 *  list for the matching `SessionInfo` and swap the detail column over to it. */
export interface PendingSpawn extends NewSessionCreated {
  /** The prompt that was sent, echoed as the first bubble of the starting
   *  pane until the real transcript replaces it. */
  prompt?: string;
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

/** What the detail column shows. Split out of the component because the one
 *  thing that went wrong here is unreachable from a render test: the pane used
 *  to pick its branch inline, and an `openId` naming a session the scan cannot
 *  see fell past every branch into the resting state — the new-session
 *  composer. So "n pending decision cards" landed on "new session composer" with nothing on screen
 *  saying why, which is exactly what the user hit on fleet-cloud on 2026-09-09. */
export type PanePlan =
  | { kind: "session" }
  /** The card is rendered right here: the pane is where every card gets
   *  answered, and this session has no transcript to pin it under. */
  | { kind: "orphan-card"; sessionId: string; decisions: PendingDecision[] }
  /** Nothing to show and nothing waiting: say why the pane is empty. */
  | { kind: "orphan-note"; sessionId: string }
  | { kind: "draft" }
  /** A restored id before the first scan lands: it cannot be told apart from
   *  an orphan yet, and flashing the composer until it resolves read as "your
   *  session is gone" for a beat on every cold start. */
  | { kind: "loading" }
  | { kind: "resting" };

export function panePlan(args: {
  openId: string | null;
  activeSession: SessionInfo | null;
  scanReady: boolean;
  decisions: PendingDecision[];
}): PanePlan {
  const { openId, activeSession, scanReady, decisions } = args;
  if (activeSession) return { kind: "session" };
  if (openId === DRAFT_ID) return { kind: "draft" };
  if (openId == null) return { kind: "resting" };
  // Before the first scan lands every id looks orphaned, so hold a placeholder
  // rather than accusing a session that is simply not loaded yet.
  if (!scanReady) return { kind: "loading" };
  const mine = decisions.filter((d) => d.request.sessionId === openId);
  return mine.length > 0
    ? { kind: "orphan-card", sessionId: openId, decisions: mine }
    : { kind: "orphan-note", sessionId: openId };
}

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

// timeAgo / formatRunning / renderSnippet / sessionEq and the SessionRow
// component itself moved to ./SessionRow (the shared row atom) so every rail
// renders the identical row without duplicating it here.

/**
 * The one gate that decides what the task page lists — deliberately taking no
 * view mode, because it must not vary by mode. Simplified mode originally
 * passed `sessions` through unfiltered, which let every scanned transcript onto
 * the page, including the `subagents/agent-*.jsonl` transcripts `scan.rs`
 * deliberately collects: one fan-out turn buried the list under a dozen rows
 * each titled with a subagent's closing sentence.
 */
export function taskListSessions(sessions: SessionInfo[]): SessionInfo[] {
  return sessions.filter(isFleetOwnedTask);
}

/**
 * Reorder `rows` (the live filtered+sorted list) back into a frozen id order,
 * used to hold the list still while the pointer is parked over it. Rows named in
 * `frozenOrder` keep that order (their live content is preserved — we resolve
 * each id against `rows`); rows absent from the snapshot — freshly spawned, or
 * newly matching the filter — are appended in their natural sort position rather
 * than spliced into the frozen block. Ids in `frozenOrder` whose row has since
 * dropped out of `rows` simply fall away. `null` = no freeze, pass through.
 */
export function applyFrozenOrder<T extends { id: string } | { key: string }>(
  rows: T[],
  frozenOrder: string[] | null,
): T[] {
  if (!frozenOrder) return rows;
  const keyOf = (r: T) => ("key" in r ? r.key : r.id);
  const byKey = new Map(rows.map((r) => [keyOf(r), r]));
  const seen = new Set<string>();
  const ordered: T[] = [];
  for (const k of frozenOrder) {
    const r = byKey.get(k);
    if (r) {
      ordered.push(r);
      seen.add(k);
    }
  }
  for (const r of rows) if (!seen.has(keyOf(r))) ordered.push(r);
  return ordered;
}


/** A row-menu edit shown ahead of the scan (see `optimistic` in HistoryView). */
export type OptimisticPatch = Partial<Pick<SessionInfo, "userMark" | "titleOverride">>;

/** One pending row-menu edit; `done` once its backend write has returned. */
interface OptimisticEntry {
  patch: OptimisticPatch;
  done: boolean;
}

/** Overlay pending row-menu edits on the scanned list. Returns the input
 *  untouched when nothing is pending so downstream memos keep their identity. */
export function applyOptimistic(
  sessions: SessionInfo[],
  entries: Record<string, { patch: OptimisticPatch }>,
): SessionInfo[] {
  if (Object.keys(entries).length === 0) return sessions;
  return sessions.map((s) => (entries[s.id] ? { ...s, ...entries[s.id].patch } : s));
}

/** Whether the scan already carries every field of `patch`. */
export function patchSettled(s: SessionInfo, patch: OptimisticPatch): boolean {
  return (Object.keys(patch) as (keyof OptimisticPatch)[]).every(
    (k) => (s[k] ?? null) === (patch[k] ?? null),
  );
}

/**
 * History page: sessions launched via the "New Session" button, as a master-detail
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
  const scannedSessions = useSessionsStore((s) => s.sessions);
  const scanReady = useSessionsStore((s) => s.scanReady);
  // Row-menu edits (mark, rename) applied ahead of the scan that will confirm
  // them, the way the row's own MarkControl already behaves. Without this the
  // menu closed and the row kept its old title / mark until the next scan, so
  // the click looked ignored. Each entry is dropped once the scan agrees with
  // it, or when the write fails (reverting the row).
  const [optimistic, setOptimistic] = useState<Record<string, OptimisticEntry>>({});
  const sessions = useMemo(
    () => applyOptimistic(scannedSessions, optimistic),
    [scannedSessions, optimistic],
  );
  const scannedRef = useRef(scannedSessions);
  scannedRef.current = scannedSessions;
  // Drop entries the scan has caught up with — or whose write has finished, so
  // an edit that changed nothing (renamed to the same title) cannot linger and
  // later mask a change made somewhere else.
  const pruneOptimistic = useCallback((scan: SessionInfo[]) => {
    setOptimistic((prev) => {
      const ids = Object.keys(prev);
      if (ids.length === 0) return prev;
      const byId = new Map(scan.map((s) => [s.id, s]));
      const kept = Object.fromEntries(
        Object.entries(prev).filter(([id, entry]) => {
          const s = byId.get(id);
          return s != null && !entry.done && !patchSettled(s, entry.patch);
        }),
      );
      return Object.keys(kept).length === ids.length ? prev : kept;
    });
  }, []);
  useEffect(() => pruneOptimistic(scannedSessions), [scannedSessions, pruneOptimistic]);
  const patchOptimistic = useCallback(
    (id: string, patch: OptimisticPatch, write: Promise<unknown>) => {
      setOptimistic((prev) => ({
        ...prev,
        [id]: { patch: { ...prev[id]?.patch, ...patch }, done: false },
      }));
      write.then(
        () => {
          // Held until the next scan confirms it; dropped now if it already has.
          setOptimistic((prev) =>
            prev[id] ? { ...prev, [id]: { ...prev[id], done: true } } : prev,
          );
          const s = scannedRef.current.find((x) => x.id === id);
          if (s && patchSettled(s, patch)) pruneOptimistic(scannedRef.current);
        },
        () =>
          // Failed: put the row back the way the scan has it.
          setOptimistic((prev) => {
            const { [id]: _dropped, ...rest } = prev;
            return rest;
          }),
      );
    },
    [pruneOptimistic],
  );

  // Rail filters live in the store, not here: this component is unmounted every
  // time `viewMode` leaves "history", which would otherwise reset them behind
  // the user's back (a waiting-input alert forcing setViewMode("list") is enough).
  const query = useUIStore((s) => s.historyQuery);
  const setQuery = useUIStore((s) => s.setHistoryQuery);
  // Project scope: one repository root, or null for every project. Kept apart
  // from the query on purpose — the full-text search would otherwise also pull
  // in every session elsewhere that merely mentions the project's name.
  const workspaceFilter = useUIStore((s) => s.historyWorkspaceFilter);
  const setWorkspaceFilter = useUIStore((s) => s.setHistoryWorkspaceFilter);
  const scopedPath = workspaceFilter === "all" ? null : workspaceFilter;
  const setScope = useCallback(
    (path: string | null) => setWorkspaceFilter(path ?? "all"),
    [setWorkspaceFilter],
  );
  // Text typed into the project menu's filter box; cleared on every open.
  const [projectDraft, setProjectDraft] = useState("");
  // Re-render on a slow tick so the relative "last updated" and the live
  // Elapsed-runtime durations keep counting even when no scan lands (a waiting-input
  // session can sit idle for minutes). 30s granularity matches the minute-level
  // display without churning the list.
  const [nowTick, setNowTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setNowTick((n) => n + 1), 30_000);
    return () => clearInterval(id);
  }, []);
  // Folded state sections ("done" starts folded). In the store, and on disk,
  // for the same unmount reason as the filters above.
  const collapsedSections = useUIStore((s) => s.homeCollapsedSections);
  const toggleSection = useUIStore((s) => s.toggleHomeSection);
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
  // The "+New Session" flow is the draft the column holds (`openId === DRAFT_ID`):
  // the pane renders the compose form, and once the form spawns a session,
  // `pending` flips it to a "starting…" spinner until the scan surfaces the
  // session and the column switches to it.
  const [pending, setPending] = useState<PendingSpawn | null>(null);
  const [startTimedOut, setStartTimedOut] = useState(false);
  // The prompt of the session the column just switched to from the composer.
  // The switch now happens on the backend's provisional row (`fresh_spawns`),
  // seconds before the CLI writes the prompt into the transcript, so the pane
  // echoes it meanwhile.
  const [launchPrompt, setLaunchPrompt] = useState<{ id: string; prompt: string } | null>(null);
  // The step before `pending`: send was pressed and `spawn_new_claude_session`
  // has not answered yet. The pane shows the same "starting…" view from this
  // moment on — the form used to sit greyed out for the whole invoke — while
  // the form itself stays mounted, hidden, because it owns the draft and the
  // invoke, and on failure it comes straight back with both intact.
  const [launching, setLaunching] = useState<NewSessionLaunch | null>(null);
  // Mirrors `launching` for the form's callbacks, which close over the render
  // that submitted. Cleared when the user backs out mid-spawn, so a late
  // success does not pull the column back onto the draft.
  const launchingRef = useRef<NewSessionLaunch | null>(null);
  // The draft and resting panes share one form slot (see `formPane`). Bumped
  // where the two used to be separate mounts — opening and closing the draft —
  // so the form still starts fresh there (focus, cleared error), but never
  // while a spawn is in flight inside it.
  const [formEpoch, setFormEpoch] = useState(0);

  // Sort freeze: while the pointer is over the list, hold the row *order* still
  // so a scan landing under the cursor can't re-sort a row out from under a
  // click. `frozenOrder` is the session-id order captured on enter; null means
  // live sorting. New sessions that appear while frozen are appended at the end
  // (see `displayRows`), never spliced into the middle. The order alone is
  // frozen — each row's *content* still updates from the live scan, and filters
  // still apply. Cleared on leave and on window blur (pointer yanked out of the
  // window without a mouseleave, same failure the per-row hover guards handle).
  const [frozenOrder, setFrozenOrder] = useState<string[] | null>(null);

  const { searching, ftsMatchPaths } = useSessionSearch(query);

  const adhocSessions = useMemo(() => taskListSessions(sessions), [sessions]);

  // The pure-chat workspace. It is no longer a mode of its own — chat sessions
  // sit in the rail like any other section — but the section is pinned to the
  // top (see `pinnedPath` below) so it never sinks among busier repos.
  const chatPath = useChatWorkspace();

  const pendingDecisions = useDecisionStore((s) => s.decisions);

  // Tasks the boss pressed "accept" on, shown as done until the scan agrees
  // (or the write fails and they come back).
  const [acceptingKeys, setAcceptingKeys] = useState<Set<string>>(() => new Set());

  // Every task and stray card, before scope and search: the project menu counts
  // over this, so it answers "which project" rather than "what is on screen".
  const allRows = useMemo(() => {
    const built = buildHomeRows(sessions, pendingDecisions, {
      isQuiet: (s) => isQuietAliveSticky(s),
      untitled: t("history.untitled"),
    });
    if (acceptingKeys.size === 0) return built;
    return built.map((r) =>
      acceptingKeys.has(r.key) && r.state === "review" ? { ...r, state: "done" as const } : r,
    );
    // nowTick: the quiet latch is time-based, so re-derive on the slow tick too.
  }, [sessions, pendingDecisions, acceptingKeys, t, nowTick]);

  // Drop optimistic accepts the scan has caught up with.
  useEffect(() => {
    if (acceptingKeys.size === 0) return;
    const settled = new Set(
      allRows
        .filter((r) => r.kind === "task" && r.tip.taskOutcome === "completed")
        .map((r) => r.key),
    );
    const live = new Set(allRows.map((r) => r.key));
    const kept = [...acceptingKeys].filter((k) => live.has(k) && !settled.has(k));
    if (kept.length !== acceptingKeys.size) setAcceptingKeys(new Set(kept));
  }, [allRows, acceptingKeys]);

  const acceptTasks = useCallback((targets: TaskRow[]) => {
    if (targets.length === 0) return;
    const keys = targets.map((r) => r.key);
    setAcceptingKeys((prev) => new Set([...prev, ...keys]));
    // One session per task is enough: the core resolves the whole relay chain.
    invoke<number>("accept_tasks", { sessionIds: targets.map((r) => r.tip.id) }).catch(
      (e) => {
        setAcceptingKeys((prev) => {
          const next = new Set(prev);
          for (const k of keys) next.delete(k);
          return next;
        });
        console.error("accept_tasks failed", e);
      },
    );
  }, []);

  // Every project on the home, for the scope menu, chat pinned first.
  const projectOptions = useMemo(() => {
    const by = new Map<string, { path: string; name: string; count: number }>();
    for (const r of allRows) {
      if (!r.projectPath) continue;
      const e = by.get(r.projectPath);
      if (e) e.count += 1;
      else by.set(r.projectPath, { path: r.projectPath, name: r.projectName, count: 1 });
    }
    return [...by.values()].sort((a, b) =>
      a.path === chatPath ? -1 : b.path === chatPath ? 1 : b.count - a.count,
    );
  }, [allRows, chatPath]);
  // The scoped project's display name. A scope whose project has left the home
  // still shows (by its folder name) so it can be seen and cleared.
  const scopedName = scopedPath
    ? (projectOptions.find((p) => p.path === scopedPath)?.name ??
      scopedPath.split(/[\\/]/).filter(Boolean).pop() ??
      scopedPath)
    : null;

  const rows = useMemo(
    () => filterRows(allRows, { scopedPath, query, ftsMatchPaths }),
    [allRows, scopedPath, query, ftsMatchPaths],
  );

  // The order shown, honouring the sort freeze: `rows` stays live; this only
  // reshuffles it back into the frozen order while the pointer is parked.
  // Sectioning keeps input order, so the freeze holds inside each section.
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const sections = useMemo(
    () => sectionRows(applyFrozenOrder(rows, frozenOrder)),
    [rows, frozenOrder],
  );

  const freezeSort = useCallback(() => {
    setFrozenOrder((prev) => prev ?? rowsRef.current.map((r) => r.key));
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

  // Stable identity: task rows are memoised, and a fresh closure each render
  // would defeat it for every row.
  const handleRowClick = useCallback(
    (s: SessionInfo) => {
      const isFtsHit = query.trim().length >= 2 && ftsMatchPaths.has(s.jsonlPath);
      openTab(s, isFtsHit ? query.trim() : null);
    },
    [query, ftsMatchPaths, openTab],
  );
  const openRow = useCallback(
    (row: HomeRow) => {
      const s = rowMenuSession(row);
      if (s) handleRowClick(s);
      else setOpenId(rowSessionId(row));
    },
    [handleRowClick],
  );

  // Row context menu. Anchor + subject are held here (not via useContextMenu) so
  // the open handler stays referentially stable — rows are memoised and a fresh
  // onContextMenu each render would re-render every row on every scan.
  const [menuAnchor, setMenuAnchor] = useState<ContextMenuAnchor | null>(null);
  const [menuSession, setMenuSession] = useState<SessionInfo | null>(null);
  const openRowMenu = useCallback((e: React.MouseEvent, row: HomeRow) => {
    // Suppress the app-wide (Settings/About/Quit) menu, same as ContextMenu's
    // own callers do.
    e.preventDefault();
    e.stopPropagation();
    const s = rowMenuSession(row);
    if (!s) return;
    setMenuSession(s);
    setMenuAnchor({ x: e.clientX, y: e.clientY });
  }, []);
  const closeRowMenu = useCallback(() => {
    setMenuAnchor(null);
    setMenuSession(null);
  }, []);

  // Manual title override ("Rename"). Held here so the dialog outlives the
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
    [t, handleRowClick, copyText, patchOptimistic],
  );

  // Back out of the composer, abandoning any in-flight spawn correlation so a
  // late scan match doesn't pull the column back onto it.
  const cancelDraft = useCallback(() => {
    launchingRef.current = null;
    setLaunching(null);
    setPending(null);
    setStartTimedOut(false);
    setOpenId(null);
    setFormEpoch((n) => n + 1);
  }, []);

  // "+New Session" → put the composer in the pane.
  const handleNewSession = () => {
    if (openId !== DRAFT_ID && !launchingRef.current) setFormEpoch((n) => n + 1);
    setOpenId(DRAFT_ID);
  };

  // Schedule page "Create" shortcut: seed the new-session composer with a
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
    // page's "Run Now" fills workspace/model/effort/tool so the draft opens as
    // a ready-to-send copy of the task; the "Create" shortcut passes only prompt.
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

  // Send pressed: the pane flips to the "starting…" view now, not when the
  // spawn returns.
  const handleLaunching = useCallback((launch: NewSessionLaunch) => {
    launchingRef.current = launch;
    setLaunching(launch);
  }, []);

  // Spawn refused: show the form again; it already holds the draft and error.
  const handleLaunchFailed = useCallback(() => {
    launchingRef.current = null;
    setLaunching(null);
  }, []);

  // Form spawned the process: keep the "starting…" view and start polling for
  // the session. Snapshot the ad-hoc session ids that exist *now* so the poller
  // can tell the new session apart from the workspace's existing ones (pid
  // can't — see matchSpawnedSession). The column moves onto the draft (it may
  // have been the resting composer) *before* arming the correlation in the
  // same batch — the spawn poller gives up unless it is there.
  const handleCreated = (info: NewSessionCreated) => {
    const launch = launchingRef.current;
    // The user backed out while the spawn was in flight. The session still
    // shows up in the rail; it just does not take over the column.
    if (!launch) return;
    launchingRef.current = null;
    setLaunching(null);
    setOpenId(DRAFT_ID);
    setPending({
      ...info,
      prompt: launch.prompt,
      knownIds: new Set(adhocSessions.map((s) => s.id)),
    });
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
      if (pending.prompt) setLaunchPrompt({ id: match.id, prompt: pending.prompt });
      setOpenId(match.id);
      setQueryById({ [match.id]: null });
      setPending(null);
      setStartTimedOut(false);
    }
  }, [adhocSessions, pending, openId]);

  // Surface an escape hatch if the spawn takes unusually long — to answer, or
  // to show up in the scan. Each phase gets its own window.
  const startPhase = launching ? "launch" : pending ? "scan" : null;
  useEffect(() => {
    setStartTimedOut(false);
    if (!startPhase) return;
    const id = setTimeout(() => setStartTimedOut(true), START_TIMEOUT_MS);
    return () => clearTimeout(id);
  }, [startPhase]);

  const activeSession = useMemo(
    () => (openId == null || openId === DRAFT_ID ? null : sessionById.get(openId) ?? null),
    [openId, sessionById],
  );

  const pane = useMemo(
    () =>
      panePlan({
        openId,
        activeSession,
        scanReady,
        decisions: pendingDecisions,
      }),
    [openId, activeSession, scanReady, pendingDecisions],
  );

  // Where the composer lives: the draft (until a spawn is pending) or the
  // resting pane. One slot for both, so starting from the resting composer and
  // then pressing "+New Session" mid-spawn does not remount the form and lose
  // the in-flight invoke's outcome.
  const formPane = (pane.kind === "draft" && !pending) || pane.kind === "resting";
  const showStarting = launching != null || (pane.kind === "draft" && pending != null);
  const startingPrompt = launching?.prompt ?? pending?.prompt;

  // Navigated off the composer while the spawn was in flight (a rail click, a
  // notification): the form is gone, so stop showing the starting view and
  // let a late answer fall on the floor (see `handleCreated`).
  useEffect(() => {
    if (launching && !formPane) {
      launchingRef.current = null;
      setLaunching(null);
    }
  }, [launching, formPane]);

  // The row the detail column is showing: the task (or stray card) that owns
  // the open session, so opening a sub-task still lights its task.
  const activeRow = useMemo(
    () => (openId && openId !== DRAFT_ID ? rowOfSession(allRows, openId) : null),
    [openId, allRows],
  );
  const activeRowKey = activeRow?.key ?? null;
  // The task's other cards — its sub-tasks' — answered from the same dock as
  // the open conversation's own.
  const taskCards = useMemo(
    () => (activeRow ? rowDecisions(activeRow, pendingDecisions, openId) : null),
    [activeRow, pendingDecisions, openId],
  );

  // A fresh window lands on the first task waiting on you, once the first scan
  // has placed the tasks. Restoring last run's pane only wins when nothing is.
  const landedRef = useRef(false);
  useEffect(() => {
    if (!scanReady || landedRef.current) return;
    landedRef.current = true;
    const first = firstNeedsYou(sections.flatMap((sec) => sec.rows));
    if (first) openRow(first);
  }, [scanReady, sections, openRow]);

  // Answering the open task's last card moves on to the next task waiting on
  // you — the inbox is worked through, not clicked through.
  const openCardCount = activeRow ? rowDecisions(activeRow, pendingDecisions).length : 0;
  const prevCardCountRef = useRef(openCardCount);
  const prevRowKeyRef = useRef(activeRowKey);
  useEffect(() => {
    const sameRow = prevRowKeyRef.current === activeRowKey;
    const drained = sameRow && prevCardCountRef.current > 0 && openCardCount === 0;
    prevCardCountRef.current = openCardCount;
    prevRowKeyRef.current = activeRowKey;
    if (!drained) return;
    const next = nextNeedsYou(sections.flatMap((sec) => sec.rows), activeRowKey);
    if (next) openRow(next);
  }, [openCardCount, activeRowKey, sections, openRow]);

  // "What should get done?" — Enter carries the words into the new-task
  // composer, which asks for the project and sends.
  const [launchText, setLaunchText] = useState("");
  const launchInputRef = useRef<HTMLInputElement>(null);
  const submitLaunch = () => {
    const prompt = launchText.trim();
    if (prompt) useComposerDraftStore.getState().patchDraft("new", { prompt });
    setLaunchText("");
    handleNewSession();
  };
  // ⌘N / Ctrl+N from anywhere on the page focuses it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "n") {
        e.preventDefault();
        launchInputRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const isMac = document.documentElement.getAttribute("data-platform") !== "windows";

  return (
    <PageShell
      view="history"
      detailKey={
        showStarting
          ? "starting"
          : pane.kind === "session" && activeSession
            ? activeSession.id
            : pane.kind
      }
      title={t("history.title", "任务")}
      // Tasks on screen, not sessions. Nothing before the first scan: a 0 there
      // reads as "you have no tasks".
      count={scanReady ? rows.length : null}
      countLoading={!scanReady}
      search={{
        value: query,
        onChange: setQuery,
        placeholder: t("history.search_placeholder", "搜索标题、计划、全文…"),
        busy: searching,
      }}
      secondary={
        <>
        <div className={styles.rail_launch} data-wizard="new-session-btn">
          <div
            className={`${styles.launch} ${openId === DRAFT_ID ? styles.launch_active : ""}`}
          >
            <Plus size={14} strokeWidth={2} className={styles.launch_icon} />
            <input
              ref={launchInputRef}
              type="text"
              className={styles.launch_input}
              value={launchText}
              onChange={(e) => setLaunchText(e.target.value)}
              onKeyDown={(e) => {
                // An IME's Enter commits the composition, not the task.
                if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  submitLaunch();
                }
              }}
              placeholder={t("home.launch_placeholder")}
              title={t("home.launch_hint")}
              aria-label={t("home.launch_placeholder")}
              spellCheck={false}
            />
            <kbd className={styles.launch_kbd}>{isMac ? "⌘N" : "Ctrl N"}</kbd>
          </div>
        </div>
        <div className={styles.controls}>
          <div className={styles.scope_row}>
            <PillMenu
              placement="below"
              className={styles.scope_menu}
              icon={<Folder size={13} strokeWidth={1.8} />}
              label={scopedName ?? t("history.all_projects", "全部项目")}
              title={scopedPath ?? t("history.all_projects", "全部项目")}
              testId="project-scope-pill"
              onOpen={() => setProjectDraft("")}
              menuHeader={() =>
                projectOptions.length > 8 ? (
                  <div className={pillStyles.menu_header}>
                    <input
                      type="text"
                      className={pillStyles.menu_header_input}
                      value={projectDraft}
                      onChange={(e) => setProjectDraft(e.target.value)}
                      placeholder={t("history.project_filter_placeholder", "筛选项目…")}
                      spellCheck={false}
                      autoFocus
                    />
                  </div>
                ) : null
              }
              items={[
                {
                  id: "all",
                  label: t("history.all_projects", "全部项目"),
                  checked: scopedPath == null,
                  onSelect: () => setScope(null),
                },
                ...projectOptions
                  .filter(
                    (p) =>
                      !projectDraft.trim() ||
                      p.name.toLowerCase().includes(projectDraft.trim().toLowerCase()),
                  )
                  .map((p) => ({
                    id: p.path,
                    label: p.name,
                    sub: t("history.project_task_count", "{{count}} 个任务", { count: p.count }),
                    checked: p.path === scopedPath,
                    onSelect: () => setScope(p.path),
                  })),
              ]}
            />
            {scopedPath && (
              <button
                type="button"
                className={styles.scope_clear}
                onClick={() => setScope(null)}
                aria-label={t("history.unfocus_workspace", "显示全部项目")}
                title={t("history.unfocus_workspace", "显示全部项目")}
              >
                <X size={12} strokeWidth={2.2} />
              </button>
            )}
          </div>
        </div>

        <div
          className={styles.list}
          onMouseEnter={freezeSort}
          onMouseLeave={thawSort}
        >
          {!scanReady ? (
            <SkeletonList rows={8} avatar rowHeight={48} className={styles.rail_skeleton} />
          ) : rows.length === 0 ? (
            <div className={styles.empty}>
              {allRows.length === 0 ? t("home.empty") : t("home.no_match")}
            </div>
          ) : (
            <TaskList
              sections={sections}
              activeKey={activeRowKey}
              collapsed={collapsedSections}
              onToggleSection={toggleSection}
              nowTick={nowTick}
              acceptingKeys={acceptingKeys}
              onOpen={openRow}
              onContextMenu={openRowMenu}
              onScopeProject={setScope}
              onAccept={acceptTasks}
            />
          )}
          <Presence when={Boolean(menuAnchor && menuSession)}>{menuAnchor && menuSession && (
            <ContextMenu
              anchor={menuAnchor}
              items={rowMenuItems(menuSession)}
              onClose={closeRowMenu}
            />
          )}</Presence>
          <Presence when={Boolean(renameTarget)}>{renameTarget && (
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
                const titleOverride = title === "" ? null : title;
                // The row takes the new title now; the scan confirms it (or
                // a failed write puts the old one back).
                patchOptimistic(
                  target.id,
                  { titleOverride },
                  invoke("set_session_title", {
                    sessionId: target.id,
                    workspacePath: target.workspacePath,
                    title: titleOverride,
                  }),
                );
              }}
            />
          )}</Presence>
        </div>

        </>
      }
    >
      {/* The detail column: one pane. It holds the session you picked in the
          rail, the new-session composer, or — with nothing picked — the
          composer as its resting state, because the one thing you can do from
          an empty column is start a task. */}
      <div className={styles.detail}>
        <div className={styles.detail_body}>
          {showStarting ? (
            <div className={styles.pane} data-testid="starting-pane">
              <div className={styles.starting_pane}>
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
                      <Spinner size={16} />
                      <span className={styles.starting_text}>
                        {t("new_session.starting", "正在启动会话…")}
                      </span>
                    </>
                  )}
                </div>
                {/* What the conversation will look like, with the prompt just
                    sent as its first bubble: the send visibly went through. */}
                <div className={styles.starting_body}>
                  <ConversationSkeleton userPrompt={startingPrompt} />
                </div>
              </div>
            </div>
          ) : pane.kind === "session" && activeSession ? (
            <div className={styles.pane}>
              <SessionDetail
                inline
                sessionInfo={activeSession}
                searchQuery={queryById[activeSession.id] ?? null}
                launchPrompt={
                  launchPrompt?.id === activeSession.id ? launchPrompt.prompt : null
                }
                taskCards={taskCards}
                projectName={activeRow?.projectName ?? null}
              />
            </div>
          ) : pane.kind === "loading" ? (
            /* Cold start with a restored session: hold its shape until the
               first scan says whether it still exists. */
            <div className={styles.pane} data-testid="restoring-session-pane">
              <div className={styles.starting_body}>
                <ConversationSkeleton />
              </div>
            </div>
          ) : pane.kind === "orphan-card" ? (
            /* Answering a card needs nothing from the transcript, so render it
               right here. */
            <div className={styles.pane} data-testid="orphan-session-pane">
              <div className={styles.orphan}>
                <p className={styles.orphan_note}>
                  {t(
                    "history.orphan_with_card",
                    "这个会话不在扫描范围内（记录可能已删除、已过期，或当前用户读不到），但它的卡还等着回复：",
                  )}
                </p>
                <DecisionDock cards={pane.decisions} />
              </div>
            </div>
          ) : pane.kind === "orphan-note" ? (
            <div className={styles.pane} data-testid="orphan-session-pane">
              <div className={styles.detail_starting}>
                <span className={styles.starting_text}>
                  {t(
                    "history.orphan_not_found",
                    "找不到这个会话：记录可能已删除、已过期，或当前用户读不到。",
                  )}
                </span>
                <button
                  type="button"
                  className={styles.starting_dismiss}
                  onClick={() => setOpenId(null)}
                >
                  {t("history.orphan_back", "关闭")}
                </button>
              </div>
            </div>
          ) : null}
          {formPane && (
            <div
              className={styles.pane}
              // Hidden, not unmounted, while the spawn is in flight: the form
              // owns the draft and the invoke (see `launching`).
              style={launching ? { display: "none" } : undefined}
            >
              <NewSessionForm
                key={formEpoch}
                // On the resting pane there is nothing to close back to, so no
                // `onCancel` (the × is hidden with it).
                onCancel={pane.kind === "draft" ? cancelDraft : undefined}
                onLaunching={handleLaunching}
                onLaunchFailed={handleLaunchFailed}
                onCreated={handleCreated}
              />
            </div>
          )}
        </div>
      </div>
    </PageShell>
  );
}
