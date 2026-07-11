import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { CheckCheck, Clock, FolderGit2, History, List, Plus, Search } from "lucide-react";
import { useReadStore, useSessionsStore, useUIStore } from "../store";
import { CollapsedSidebarRail } from "./CollapsedSidebarRail";
import type { SessionInfo } from "../types";
import { isFleetOwnedEntrypoint, sessionUnread } from "../types";
import { useSessionSearch } from "../hooks/useSessionSearch";
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
 *  read/unread axis is deliberately NOT filterable — only pending/done is. */
type MarkFilter = "all" | "pending" | "done";
const MARK_SEGMENTS: { key: MarkFilter; dot?: string }[] = [
  { key: "all" },
  { key: "pending", dot: "#d0a85a" },
  { key: "done" },
];

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

/** Left accent-bar colour for run status: green = agent still live, amber =
 *  waiting for input. Ended sessions get no bar (null) — the bar is a positive
 *  "this one's doing something" signal, not another dot on every row. Distinct
 *  in shape from the round review mark on the right. */
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
  const { sessions, scanReady } = useSessionsStore();
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
  const [, setNowTick] = useState(0);
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
          s.workspaceName.toLowerCase().includes(q);
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

  const handleRowClick = (s: SessionInfo) => {
    const isFtsHit = query.trim().length >= 2 && ftsMatchPaths.has(s.jsonlPath);
    setComposing(false);
    setPending(null);
    setStartTimedOut(false);
    setSelected(s);
    setDetailQuery(isFtsHit ? query.trim() : null);
  };

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
      <div className={styles.rail}>
        <div className={styles.header}>
          <span className={styles.header_title}>{t("history.title", "启动台")}</span>
          <span className={styles.header_count}>{rows.length}</span>
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
              placeholder={t("history.search_placeholder", "搜索标题、提示词、全文…")}
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
            <button
              type="button"
              className={styles.mark_all_read}
              disabled={unreadSessions.length === 0}
              onClick={() => markManyRead(unreadSessions)}
              title={t("history.mark_all_read_tip", "把所有未读会话标记为已读")}
            >
              <CheckCheck size={13} strokeWidth={1.8} />
              {t("history.mark_all_read", "全部已读")}
            </button>
          </div>
          <div
            className={styles.mark_filter}
            role="group"
            aria-label={t("history.mark_filter_label", "按标记状态筛选")}
          >
            {MARK_SEGMENTS.map((seg) => {
              const label =
                seg.key === "all"
                  ? t("history.mark_f_all", "全部")
                  : seg.key === "pending"
                    ? t("history.mark_f_pending", "进行中")
                    : t("history.mark_f_done", "已完成");
              return (
                <button
                  key={seg.key}
                  type="button"
                  className={`${styles.mark_seg} ${markFilter === seg.key ? styles.mark_seg_on : ""}`}
                  aria-pressed={markFilter === seg.key}
                  onClick={() => setMarkFilter(seg.key)}
                  title={label}
                  aria-label={`${label} (${markCounts[seg.key]})`}
                >
                  {seg.key === "all" && <List size={12} strokeWidth={1.8} />}
                  {seg.dot && (
                    <span
                      className={styles.mark_seg_dot}
                      style={{ background: seg.dot }}
                    />
                  )}
                  {seg.key === "done" && (
                    <span
                      className={`${styles.mark_seg_dot} ${styles.mark_seg_dot_done}`}
                    />
                  )}
                  <span className={styles.mark_seg_count}>{markCounts[seg.key]}</span>
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
            rows.map((s) => {
              const snippet =
                query.trim().length >= 2 ? snippetByPath.get(s.jsonlPath) : undefined;
              return (
                <div key={s.jsonlPath} className={styles.row_wrap}>
                  <button
                    type="button"
                    className={`${styles.row} ${selected?.id === s.id ? styles.row_active : ""}`}
                    onClick={() => handleRowClick(s)}
                    title={s.lastMessagePreview ?? undefined}
                  >
                    {rowBarColor(s) && (
                      <span
                        className={styles.row_bar}
                        style={{ background: rowBarColor(s)! }}
                      />
                    )}
                    {sessionUnread(s, readOverrides[s.id]) && (
                      <span
                        className={styles.unread_dot}
                        title={t("history.unread", "未读 — 有新消息")}
                        aria-label={t("history.unread", "未读 — 有新消息")}
                      />
                    )}
                    <span className={styles.row_body}>
                      <span className={styles.row_title}>
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
                            style={{ color: rowBarColor(s) ?? undefined }}
                            title={new Date(s.createdAtMs).toLocaleString()}
                          >
                            <Clock size={10} strokeWidth={1.6} />
                            {formatRunning(s.createdAtMs, t)}
                          </span>
                        )}
                        <span className={styles.row_time}>{timeAgo(s.lastActivityMs, t)}</span>
                      </span>
                      {snippet && (
                        <span className={styles.row_snippet}>{renderSnippet(snippet)}</span>
                      )}
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
            })
          )}
        </div>
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
