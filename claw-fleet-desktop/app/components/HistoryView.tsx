import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import {
  CheckCheck,
  CheckCircle2,
  Circle,
  Clock,
  FolderGit2,
  History,
  Plus,
  Search,
} from "lucide-react";
import { useReadStore, useSessionsStore, useUIStore } from "../store";
import { CollapsedSidebarRail } from "./CollapsedSidebarRail";
import type { SessionInfo } from "../types";
import { isFleetOwnedEntrypoint, sessionUnread } from "../types";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import { NewSessionForm, type NewSessionCreated } from "./NewSessionForm";
import { SessionDetail } from "./SessionDetail";
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

const LIVE_STATUSES = new Set([
  "thinking", "executing", "streaming", "processing",
  "waitingInput", "active", "delegating",
]);

/** Segments for the pending/done filter. "all" shows everything; the other two
 *  map to the binary mark buckets (unmarked collapses to "pending"). The
 *  read/unread axis is deliberately NOT filterable — only pending/done is.
 *
 *  The two bucket segments render the exact icons `MarkControl` puts on the row
 *  (hollow circle = pending, green check = done) so a segment reads as "show me
 *  the rows wearing this icon". "all" is spelled out in words instead — any
 *  third icon here just competes with those two for meaning. */
type MarkFilter = "all" | "pending" | "done";
const MARK_SEGMENTS: MarkFilter[] = ["all", "pending", "done"];

/** Which bucket a session falls in — `done` is explicit, everything else is
 *  pending. */
function markBucket(s: SessionInfo): "pending" | "done" {
  return s.userMark === "done" ? "done" : "pending";
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

/** Run-status colour: green = agent still live, amber = waiting for input.
 *  Ended sessions get nothing (null) — this is a positive "this one's doing
 *  something" signal, not another mark on every row. Drives both the left dot
 *  and the "已运行 N" runtime label, which are two views of the same fact. */
function rowBarColor(s: SessionInfo): string | null {
  if (!LIVE_STATUSES.has(s.status)) return null;
  if (s.status === "waitingInput") return "#d0a85a";
  return "#5ac88c";
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
  unread: boolean;
  /** Bumped every 30s by the parent so relative times keep advancing. Without
   *  it the memo would freeze "已运行 3 分钟" at whatever it said on mount. */
  nowTick: number;
  onClick: (s: SessionInfo) => void;
};

const SessionRow = memo(function SessionRow({
  session: s,
  snippet,
  isSelected,
  unread,
  onClick,
}: SessionRowProps) {
  const { t } = useTranslation();
  const runColor = rowBarColor(s);
  return (
    <div className={styles.row_wrap}>
      <button
        type="button"
        className={`${styles.row} ${isSelected ? styles.row_active : ""} ${unread ? styles.row_unread : ""}`}
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
                🔗 {s.handoff.hop}/{s.handoff.chainLen}
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
            <span className={styles.row_time}>{timeAgo(s.lastActivityMs, t)}</span>
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
  const collapsed = useUIStore((s) => !!s.secondarySidebarCollapsed["history"]);
  const setSecondarySidebar = useUIStore((s) => s.setSecondarySidebar);
  const {
    width: railWidth,
    isDragging: railDragging,
    onMouseDown: onRailMouseDown,
  } = useResizableWidth("history-rail-width", { min: 240, max: 640, initial: 300 });
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

  const [query, setQuery] = useState("");
  const [workspaceFilter, setWorkspaceFilter] = useState("all");
  // Re-render on a slow tick so the relative "last updated" and the live
  // "已运行" durations keep counting even when no scan lands (a waiting-input
  // session can sit idle for minutes). 30s granularity matches the minute-level
  // display without churning the list.
  const [nowTick, setNowTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setNowTick((n) => n + 1), 30_000);
    return () => clearInterval(id);
  }, []);
  // Narrow the rail to sessions whose agent is still live — same status set that
  // colours the row dot green/amber.
  const [activeOnly, setActiveOnly] = useState(false);
  // Segmented filter by manual review mark; "all" shows every bucket.
  const [markFilter, setMarkFilter] = useState<MarkFilter>("all");
  // Inline detail column selection — local to the page, deliberately NOT the
  // global useDetailStore (that one drives the drawer overlaying every view).
  const [selected, setSelected] = useState<SessionInfo | null>(null);
  const [detailQuery, setDetailQuery] = useState<string | null>(null);
  // Detail-column modes are mutually exclusive, in precedence order:
  //   pending  → "starting…" spinner while we wait for the scanner
  //   composing→ the inline new-session form
  //   selected → an existing session's SessionDetail
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

  const workspaces = useMemo(() => {
    const byPath = new Map<string, string>();
    for (const s of adhocSessions) byPath.set(s.workspacePath, s.workspaceName);
    return [...byPath.entries()].sort((a, b) => a[1].localeCompare(b[1]));
  }, [adhocSessions]);

  const { rows, markCounts } = useMemo(() => {
    const q = query.trim().toLowerCase();
    // Everything except the mark filter — the segment counts are taken over
    // this set so each count reflects how many rows its segment would reveal
    // under the current workspace / query / active filters.
    const preMark = adhocSessions
      .filter((s) => workspaceFilter === "all" || s.workspacePath === workspaceFilter)
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
      // Most recently *active* first, matching the row's displayed time — a
      // created-at order would interleave stale rows between fresh ones.
      .sort((a, b) => b.lastActivityMs - a.lastActivityMs);
    return { rows, markCounts: counts };
  }, [adhocSessions, workspaceFilter, activeOnly, query, ftsMatchPaths, markFilter]);

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

  // Stable identity: SessionRow is memoised, and a fresh closure each render
  // would defeat it for every row.
  const handleRowClick = useCallback(
    (s: SessionInfo) => {
      const isFtsHit = query.trim().length >= 2 && ftsMatchPaths.has(s.jsonlPath);
      setComposing(false);
      setPending(null);
      setStartTimedOut(false);
      setSelected(s);
      setDetailQuery(isFtsHit ? query.trim() : null);
    },
    [query, ftsMatchPaths],
  );

  // "+新会话" → swap the detail column to the inline compose form.
  const handleNewSession = () => {
    setSelected(null);
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
      setSelected(match);
      setDetailQuery(null);
      setPending(null);
      setStartTimedOut(false);
    }
  }, [adhocSessions, pending]);

  // Surface an escape hatch if the spawn takes unusually long to show up.
  useEffect(() => {
    if (!pending) {
      setStartTimedOut(false);
      return;
    }
    const id = setTimeout(() => setStartTimedOut(true), START_TIMEOUT_MS);
    return () => clearTimeout(id);
  }, [pending]);

  // Dwell-to-read: staying on a selected session for 2s marks it read. Clicking
  // away (or unmounting) before the timer fires cancels it, so a quick glance
  // doesn't clear the unread dot. Re-keyed on the selected id, not its activity,
  // so a still-streaming session can flip back to unread and get re-read on a
  // later visit — matching "new message after last read → unread".
  useEffect(() => {
    if (!selected) return;
    const target = selected;
    const id = setTimeout(() => markRead(target), 2000);
    return () => clearTimeout(id);
  }, [selected?.id, markRead]);

  return (
    <div className={styles.page}>
      {collapsed ? (
        <CollapsedSidebarRail onExpand={() => setSecondarySidebar("history", false)} />
      ) : (
      <div className={styles.rail} style={{ width: railWidth }}>
        <div className={styles.header}>
          <span className={styles.header_title}>{t("history.title", "启动台")}</span>
          <span className={styles.header_count}>{rows.length}</span>
          {/* Icon-only in the header: the rail starts at 300px and "Mark all read"
              spelled out would push the new-session button off the row in en locale. */}
          <button
            type="button"
            className={styles.header_read_btn}
            disabled={unreadSessions.length === 0}
            onClick={() => markManyRead(unreadSessions)}
            title={t("history.mark_all_read_tip", "把所有未读会话标记为已读")}
            aria-label={t("history.mark_all_read", "全部已读")}
          >
            <CheckCheck size={14} strokeWidth={1.8} />
          </button>
          <button
            type="button"
            className={`${styles.header_new_btn} ${composing ? styles.header_new_btn_active : ""}`}
            onClick={handleNewSession}
            title={t("new_session.title")}
          >
            <Plus size={12} strokeWidth={2} />
            <span>{t("new_session.button")}</span>
          </button>
        </div>

        <div className={styles.controls}>
          <div className={styles.search_wrap}>
            <Search size={13} strokeWidth={1.6} className={styles.search_icon} />
            <input
              className={styles.search_input}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("history.search_placeholder", "搜索标题、计划、全文…")}
              spellCheck={false}
            />
            {searching && <span className={styles.search_spinner} />}
          </div>
          <div className={styles.filter_row}>
            <select
              className={styles.project_select}
              value={workspaceFilter}
              onChange={(e) => setWorkspaceFilter(e.target.value)}
              title={t("history.filter_workspace", "按工作目录筛选")}
            >
              <option value="all">{t("history.all_workspaces", "全部目录")}</option>
              {workspaces.map(([path, name]) => (
                <option key={path} value={path} title={path}>{name}</option>
              ))}
            </select>
            <button
              type="button"
              className={`${styles.active_toggle} ${activeOnly ? styles.active_toggle_on : ""}`}
              aria-pressed={activeOnly}
              onClick={() => setActiveOnly((v) => !v)}
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
                isSelected={selected?.id === s.id}
                unread={sessionUnread(s, readOverrides[s.id])}
                nowTick={nowTick}
                onClick={handleRowClick}
              />
            ))
          )}
        </div>

        <ResizeHandle active={railDragging} onMouseDown={onRailMouseDown} />
      </div>
      )}

      <div className={styles.detail}>
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
        ) : selected ? (
          <SessionDetail inline sessionInfo={selected} searchQuery={detailQuery} />
        ) : (
          <div className={styles.detail_empty}>
            <History size={28} strokeWidth={1.2} />
            <span>{t("history.select_hint", "从左侧选择一个会话查看详情")}</span>
          </div>
        )}
      </div>
    </div>
  );
}
