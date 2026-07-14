import { listen } from "@tauri-apps/api/event";
import { CheckCheck, Plus, Search } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  openSettingsWindow,
  useDetailStore,
  useReadStore,
  useSessionsStore,
  useUIStore,
  type MarkFilter,
} from "../store";
import type { SessionInfo } from "../types";
import { LIVE_STATUSES, isFleetOwnedEntrypoint, sessionUnread } from "../types";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { getItem, setItem } from "../storage";
import { CHAT_HIDDEN, CHAT_ONLY, matchesWorkspaceFilter } from "./HistoryView";
import { LiteDecisionHistory } from "./LiteDecisionHistory";
import { LiteSessionCard } from "./LiteSessionCard";
import { MarkControl } from "./MarkControl";
import { NewSessionForm, type NewSessionCreated } from "./NewSessionForm";
import { SessionDetail } from "./SessionDetail";
import { StopControl, canControl } from "./StopControl";
import { TodayUsageBadge } from "./TodayUsageBadge";
import styles from "./LiteApp.module.css";

const MARK_SEGMENTS: MarkFilter[] = ["all", "pending", "done"];

/** Which mark bucket a session falls in — mirrors HistoryView.markBucket. */
function markBucket(s: SessionInfo): "pending" | "done" {
  return s.userMark === "done" ? "done" : "pending";
}

/** A row shows the stop control only when the session is controllable and
 *  actually live/has a pid — mirrors HistoryView.showsControl. */
function showsControl(s: SessionInfo): boolean {
  return canControl(s) && (s.pid !== null || LIVE_STATUSES.has(s.status));
}

/**
 * Lite mode = a phone-shaped portrait strip that mirrors the mobile app's
 * **task page** in a desktop mini-window:
 *
 *   • Task list  — the launchpad's `adhocSessions` (sessions Fleet itself
 *     spawned via the "新会话" button, i.e. `isFleetOwnedEntrypoint`), with the
 *     same filter bar as the desktop/mobile task page: search + workspace
 *     filter + active-only toggle + all/pending/done mark segments, plus inline
 *     stop/mark controls and a mark-all-read shortcut. NOT the full session
 *     list (that's the "会话页").
 *   • New session — a prominent "+ 新会话" button opens the shared
 *     `NewSessionForm` full-window; the spawned session opens automatically.
 *   • Detail page — tapping a task pushes SessionDetail over the full window;
 *     its own close button acts as the back affordance.
 *
 * Decision cards are NOT rendered here — in lite mode they always pop out as
 * the standalone `decision-float` window (see App.tsx's float bridge).
 */
export function LiteApp() {
  const { t } = useTranslation();
  const { sessions, setSessions, refresh } = useSessionsStore();
  const { open, session: openedSession } = useDetailStore();
  const { setLiteMode, liteDecisionHistorySessionId } = useUIStore();
  const readOverrides = useReadStore((s) => s.overrides);
  const markManyRead = useReadStore((s) => s.markManyRead);
  const chatPath = useChatWorkspace();
  const [ttsMuted, setTtsMuted] = useState(() => getItem("tts-muted") === "true");
  // Local (lite-scoped) filters — independent from the desktop task page's
  // persisted history* filters so toggling one window doesn't move the other.
  const [query, setQuery] = useState("");
  const [markFilter, setMarkFilter] = useState<MarkFilter>("all");
  const [workspaceFilter, setWorkspaceFilter] = useState<string>("all");
  const [activeOnly, setActiveOnly] = useState(false);
  // New-session composer + auto-open of the freshly spawned session.
  const [composing, setComposing] = useState(false);
  const [spawnedId, setSpawnedId] = useState<string | null>(null);

  const toggleTtsMuted = () => {
    const next = !ttsMuted;
    setTtsMuted(next);
    setItem("tts-muted", next ? "true" : "false");
  };

  // Keep session list flowing in lite mode (normal SessionList is unmounted).
  useEffect(() => {
    const unlistenPromise = listen<SessionInfo[]>("sessions-updated", (e) => {
      setSessions(e.payload);
    });
    refresh().catch(() => {});
    return () => {
      unlistenPromise.then((u) => u());
    };
  }, [setSessions, refresh]);

  // After spawning a new session, open its detail as soon as the scanner
  // surfaces it (matches by the pre-assigned id).
  useEffect(() => {
    if (!spawnedId) return;
    const match = sessions.find((s) => s.id === spawnedId);
    if (match) {
      open(match).catch(() => {});
      setSpawnedId(null);
    }
  }, [spawnedId, sessions, open]);

  const { ftsMatchPaths } = useSessionSearch(query);

  // Task list = launchpad adhoc sessions, filtered/sorted exactly like the
  // desktop task page (HistoryView): workspace → active-only → search → mark.
  const { rows, markCounts, workspaces, activeCount, unreadSessions } = useMemo(() => {
    const adhoc = sessions.filter(
      (s) => !s.isSubagent && isFleetOwnedEntrypoint(s.entrypoint),
    );
    // Workspace dropdown options (chat workspace is pinned separately below).
    const byPath = new Map<string, string>();
    for (const s of adhoc) {
      if (s.workspacePath === chatPath) continue;
      byPath.set(s.workspacePath, s.workspaceName);
    }
    const workspaces = [...byPath.entries()].sort((a, b) => a[1].localeCompare(b[1]));
    const activeCount = adhoc.filter((s) => LIVE_STATUSES.has(s.status)).length;
    const unreadSessions = adhoc.filter((s) => sessionUnread(s, readOverrides[s.id]));
    const q = query.trim().toLowerCase();
    const preMark = adhoc
      .filter((s) => matchesWorkspaceFilter(s, workspaceFilter, chatPath))
      .filter((s) => !activeOnly || LIVE_STATUSES.has(s.status))
      .filter((s) => {
        if (!q) return true;
        const clientMatch =
          (s.aiTitle?.toLowerCase().includes(q) ?? false) ||
          (s.slug?.toLowerCase().includes(q) ?? false) ||
          (s.lastMessagePreview?.toLowerCase().includes(q) ?? false) ||
          s.workspaceName.toLowerCase().includes(q) ||
          (s.taskPlan?.planId?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentPlan?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentTask?.toLowerCase().includes(q) ?? false);
        return clientMatch || ftsMatchPaths.has(s.jsonlPath);
      });
    const counts: Record<MarkFilter, number> = { all: preMark.length, pending: 0, done: 0 };
    for (const s of preMark) counts[markBucket(s)] += 1;
    const rows = preMark
      .filter((s) => markFilter === "all" || markBucket(s) === markFilter)
      .sort((a, b) => b.lastActivityMs - a.lastActivityMs);
    return { rows, markCounts: counts, workspaces, activeCount, unreadSessions };
  }, [sessions, query, ftsMatchPaths, markFilter, workspaceFilter, activeOnly, chatPath, readOverrides]);

  // A workspace filter naming a directory whose sessions have all aged out would
  // strand an empty list behind a blank select — fall back to "all".
  useEffect(() => {
    if (workspaceFilter === "all" || workspaceFilter === CHAT_ONLY || workspaceFilter === CHAT_HIDDEN) return;
    if (workspaceFilter === chatPath) return;
    if (!workspaces.some(([path]) => path === workspaceFilter)) setWorkspaceFilter("all");
  }, [workspaces, workspaceFilter, chatPath]);

  const onCreated = (info: NewSessionCreated) => {
    setComposing(false);
    if (info.sessionId) setSpawnedId(info.sessionId);
  };

  const segmentLabel = (seg: MarkFilter) =>
    seg === "all"
      ? t("history.mark_f_all", "全部")
      : seg === "pending"
        ? t("history.mark_f_pending", "进行中")
        : t("history.mark_f_done", "已完成");

  return (
    <div className={styles.lite}>
      <div className={styles.drag_bar} data-tauri-drag-region>
        <span className={styles.drag_title} data-tauri-drag-region>
          {t("title")}
        </span>
        <TodayUsageBadge inline />
        <button
          className={styles.icon_btn}
          title={t(ttsMuted ? "lite.unmute" : "lite.mute")}
          onClick={toggleTtsMuted}
        >
          {ttsMuted ? (
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
              <path d="M8 3L5 6H2v4h3l3 3z" />
              <line x1="11" y1="6" x2="15" y2="10" />
              <line x1="15" y1="6" x2="11" y2="10" />
            </svg>
          ) : (
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
              <path d="M8 3L5 6H2v4h3l3 3z" />
              <path d="M11.5 5.5a3.5 3.5 0 0 1 0 5" />
              <path d="M13.5 3.5a6 6 0 0 1 0 9" />
            </svg>
          )}
        </button>
        <button
          className={styles.icon_btn}
          title={t("settings.title")}
          onClick={openSettingsWindow}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="8" cy="8" r="1.5" />
            <path d="M6.7 1.2l-.4 1.6a5 5 0 0 0-1.5.9L3.3 3.2 1.9 5.6l1.2 1.1a5 5 0 0 0 0 1.7l-1.2 1.1 1.4 2.4 1.5-.5a5 5 0 0 0 1.5.9l.4 1.6h2.6l.4-1.6a5 5 0 0 0 1.5-.9l1.5.5 1.4-2.4-1.2-1.1a5 5 0 0 0 0-1.7l1.2-1.1-1.4-2.4-1.5.5a5 5 0 0 0-1.5-.9L9.3 1.2z" />
          </svg>
        </button>
        <button
          className={styles.icon_btn}
          title={t("lite.exit")}
          onClick={() => setLiteMode(false)}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
            <path d="M4 6 L4 3 L13 3 L13 13 L4 13 L4 10" />
            <path d="M9 8 L2 8 M5 5 L2 8 L5 11" />
          </svg>
        </button>
      </div>

      {composing ? (
        <div className={styles.detail_area}>
          <NewSessionForm onCreated={onCreated} onCancel={() => setComposing(false)} />
        </div>
      ) : liteDecisionHistorySessionId ? (
        <div className={styles.detail_area}>
          <LiteDecisionHistory sessionId={liteDecisionHistorySessionId} />
        </div>
      ) : openedSession ? (
        <div className={styles.detail_area}>
          <SessionDetail lite />
        </div>
      ) : (
        <div className={styles.body}>
          {/* Toolbar — search + workspace/active filters + mark segments. */}
          <div className={styles.toolbar}>
            <label className={styles.search}>
              <Search size={13} strokeWidth={1.6} className={styles.search_icon} />
              <input
                className={styles.search_input}
                type="text"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={t("history.search_placeholder", "搜索标题、计划、全文…")}
              />
            </label>

            <div className={styles.filter_row}>
              <select
                className={styles.ws_select}
                value={workspaceFilter}
                onChange={(e) => setWorkspaceFilter(e.target.value)}
                title={t("history.filter_workspace", "按工作目录筛选")}
              >
                <option value="all">{t("history.all_workspaces", "全部目录")}</option>
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
                onClick={() => setActiveOnly((v) => !v)}
                title={t("history.filter_active_tip", "只显示仍在运行或等待输入的会话")}
              >
                <span className={styles.active_dot} />
                {t("history.only_active", "仅活跃")}
                <span className={styles.active_count}>{activeCount}</span>
              </button>
              {unreadSessions.length > 0 && (
                <button
                  type="button"
                  className={styles.mark_all_read}
                  onClick={() => markManyRead(unreadSessions)}
                  title={t("history.mark_all_read", "全部已读") + ` (${unreadSessions.length})`}
                >
                  <CheckCheck size={14} strokeWidth={1.8} />
                  <span className={styles.mark_all_read_count}>{unreadSessions.length}</span>
                </button>
              )}
            </div>

            <div className={styles.segments}>
              {MARK_SEGMENTS.map((seg) => (
                <button
                  key={seg}
                  className={`${styles.segment} ${markFilter === seg ? styles.segment_active : ""}`}
                  onClick={() => setMarkFilter(seg)}
                >
                  {segmentLabel(seg)}
                  <span className={styles.segment_count}>{markCounts[seg]}</span>
                </button>
              ))}
            </div>
          </div>

          {/* New session — the primary action. */}
          <button
            className={styles.new_session}
            onClick={() => setComposing(true)}
            title={t("new_session.title")}
          >
            <Plus size={15} strokeWidth={2} />
            <span>{t("new_session.button")}</span>
          </button>

          {/* Task list — launchpad adhoc sessions with inline stop/mark. */}
          <div className={styles.list}>
            {rows.length === 0 ? (
              <p className={styles.empty}>
                {query.trim()
                  ? t("history.no_match", "没有匹配的会话")
                  : t("history.empty_hint", "还没有会话，点上方新建一个")}
              </p>
            ) : (
              rows.map((s) => (
                <div key={s.jsonlPath} className={styles.task_row}>
                  <LiteSessionCard session={s} onClick={() => open(s).catch(() => {})} />
                  <div className={styles.task_actions}>
                    {showsControl(s) && <StopControl session={s} />}
                    <MarkControl session={s} />
                  </div>
                </div>
              ))
            )}
          </div>
        </div>
      )}
    </div>
  );
}
