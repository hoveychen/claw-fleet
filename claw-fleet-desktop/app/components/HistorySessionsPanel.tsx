import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { FolderGit2, RotateCcw, Search, X } from "lucide-react";
import {
  useDetailStore,
  useProjectsStore,
  useSessionsStore,
  type FleetSession,
} from "../store";
import type { SessionInfo } from "../types";
import { useSessionSearch } from "../hooks/useSessionSearch";
import styles from "./HistorySessionsPanel.module.css";

const LIVE_STATUSES = new Set([
  "thinking", "executing", "streaming", "processing",
  "waitingInput", "active", "delegating",
]);

function timeAgo(ms: number, t: (k: string, opts?: Record<string, unknown>) => string): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("just_now");
  if (diff < 3_600_000) return t("m_ago", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("h_ago", { n: Math.floor(diff / 3_600_000) });
  return t("d_ago", { n: Math.floor(diff / 86_400_000) });
}

function firstLine(text: string, max = 90): string {
  const line = text.split("\n").find((l) => l.trim().length > 0)?.trim() ?? "";
  return line.length > max ? `${line.slice(0, max)}…` : line;
}

/** Green = agent still live, amber = waiting for input, gray = ended. */
function rowDotColor(live: SessionInfo | undefined): string {
  if (!live || !LIVE_STATUSES.has(live.status)) return "#6b6b72";
  if (live.status === "waitingInput") return "#d0a85a";
  return "#5ac88c";
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

interface HistoryRow {
  fs: FleetSession;
  live: SessionInfo | undefined;
  projectName: string | null;
  title: string;
}

/**
 * Secondary sidebar: the durable log of Fleet-spawned sessions
 * (fleet-sessions.json via list_fleet_sessions), newest first, with a text
 * search (client substring + transcript FTS) and a per-project filter.
 *
 * Rows whose transcript is still on disk join to the live SessionInfo scan
 * and open in SessionDetail on click; rows whose jsonl is gone expand
 * inline to show the recorded metadata plus a resume box.
 */
export function HistorySessionsPanel({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const projects = useProjectsStore((s) => s.projects);
  const fleetSessions = useProjectsStore((s) => s.fleetSessions);
  const loaded = useProjectsStore((s) => s.loaded);
  const sessions = useSessionsStore((s) => s.sessions);
  const { session: viewedSession, open } = useDetailStore();

  const [query, setQuery] = useState("");
  const [projectFilter, setProjectFilter] = useState("all");
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [resumeText, setResumeText] = useState("");
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [resumeError, setResumeError] = useState<string | null>(null);

  // The sidebar's 5s poller only feeds the projects store while the
  // projects feature flag is on — poll independently so this panel stays
  // fresh either way.
  useEffect(() => {
    useProjectsStore.getState().refresh();
    const interval = setInterval(() => {
      if (document.visibilityState !== "hidden") {
        useProjectsStore.getState().refresh();
      }
    }, 5000);
    return () => clearInterval(interval);
  }, []);

  const { searching, ftsMatchPaths, snippetByPath } = useSessionSearch(query);

  const liveById = useMemo(
    () => new Map(sessions.map((s) => [s.id, s])),
    [sessions],
  );
  const projectNameById = useMemo(
    () => new Map(projects.map((p) => [p.id, p.name])),
    [projects],
  );

  const rows = useMemo<HistoryRow[]>(() => {
    const q = query.trim().toLowerCase();
    return fleetSessions
      .filter((fs) => projectFilter === "all" || fs.projectId === projectFilter)
      .map((fs) => {
        const live = liveById.get(fs.id);
        return {
          fs,
          live,
          projectName: projectNameById.get(fs.projectId) ?? null,
          title: live?.aiTitle ?? live?.slug ?? firstLine(fs.prompt),
        };
      })
      .filter((r) => {
        if (!q) return true;
        const clientMatch =
          r.title.toLowerCase().includes(q) ||
          r.fs.prompt.toLowerCase().includes(q) ||
          (r.projectName?.toLowerCase().includes(q) ?? false) ||
          r.fs.workspace.toLowerCase().includes(q);
        return clientMatch || (r.live ? ftsMatchPaths.has(r.live.jsonlPath) : false);
      })
      .sort((a, b) => b.fs.createdAt - a.fs.createdAt);
  }, [fleetSessions, projectFilter, query, liveById, projectNameById, ftsMatchPaths]);

  const handleRowClick = (row: HistoryRow) => {
    if (row.live) {
      const isFtsHit = query.trim().length >= 2 && ftsMatchPaths.has(row.live.jsonlPath);
      open(row.live, isFtsHit ? query.trim() : undefined);
      return;
    }
    // Transcript gone — no SessionDetail to open; expand the recorded
    // metadata inline instead.
    setExpandedId((prev) => (prev === row.fs.id ? null : row.fs.id));
    setResumeText("");
    setResumeError(null);
  };

  const handleResume = async (fs: FleetSession) => {
    const prompt = resumeText.trim();
    if (!prompt || resumingId) return;
    setResumingId(fs.id);
    setResumeError(null);
    try {
      await invoke("resume_fleet_session", {
        sessionId: fs.id,
        followUpPrompt: prompt,
      });
      setExpandedId(null);
      setResumeText("");
    } catch (e) {
      setResumeError(String((e as { message?: string })?.message ?? e));
    } finally {
      setResumingId(null);
    }
  };

  return (
    <div className={styles.panel}>
      <div className={styles.header}>
        <span className={styles.header_title}>{t("history.title", "历史会话")}</span>
        <span className={styles.header_count}>{rows.length}</span>
        <button
          type="button"
          className={styles.close_btn}
          onClick={onClose}
          title={t("common.close", "关闭")}
          aria-label={t("common.close", "关闭")}
        >
          <X size={14} strokeWidth={1.6} />
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
        <select
          className={styles.project_select}
          value={projectFilter}
          onChange={(e) => setProjectFilter(e.target.value)}
          title={t("history.filter_project", "按项目筛选")}
        >
          <option value="all">{t("history.all_projects", "全部项目")}</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>{p.name}</option>
          ))}
        </select>
      </div>

      <div className={styles.list}>
        {!loaded ? (
          <div className={styles.empty}>{t("scanning")}</div>
        ) : rows.length === 0 ? (
          <div className={styles.empty}>
            {fleetSessions.length === 0
              ? t("history.empty", "还没有通过 Fleet 创建的会话")
              : t("history.no_match", "没有匹配的会话")}
          </div>
        ) : (
          rows.map((row) => {
            const expanded = expandedId === row.fs.id && !row.live;
            const snippet =
              query.trim().length >= 2 && row.live
                ? snippetByPath.get(row.live.jsonlPath)
                : undefined;
            return (
              <div key={row.fs.id} className={styles.row_wrap}>
                <button
                  type="button"
                  className={`${styles.row} ${viewedSession?.id === row.fs.id ? styles.row_active : ""}`}
                  onClick={() => handleRowClick(row)}
                  title={row.fs.prompt}
                >
                  <span
                    className={styles.row_dot}
                    style={{ background: rowDotColor(row.live) }}
                  />
                  <span className={styles.row_body}>
                    <span className={styles.row_title}>
                      {row.title || t("history.untitled", "（无标题）")}
                    </span>
                    <span className={styles.row_meta}>
                      {row.projectName && (
                        <span className={styles.row_project}>
                          <FolderGit2 size={10} strokeWidth={1.6} />
                          {row.projectName}
                        </span>
                      )}
                      {(row.fs.sessionKind === "master" || row.fs.sessionKind === "worker") && (
                        <span className={styles.row_kind}>{row.fs.sessionKind}</span>
                      )}
                      <span className={styles.row_time}>{timeAgo(row.fs.createdAt, t)}</span>
                    </span>
                    {snippet && (
                      <span className={styles.row_snippet}>{renderSnippet(snippet)}</span>
                    )}
                  </span>
                </button>
                {expanded && (
                  <div className={styles.detail_fallback}>
                    <div className={styles.fallback_note}>
                      {t("history.missing_transcript", "转录文件已不在磁盘上，仅剩登记信息")}
                    </div>
                    <div className={styles.fallback_prompt}>{row.fs.prompt}</div>
                    {row.fs.note && (
                      <div className={styles.fallback_prompt}>{row.fs.note}</div>
                    )}
                    <div className={styles.resume_box}>
                      <textarea
                        className={styles.resume_input}
                        value={resumeText}
                        onChange={(e) => setResumeText(e.target.value)}
                        placeholder={t("history.resume_placeholder", "输入追问提示词后恢复会话…")}
                        rows={2}
                      />
                      <button
                        type="button"
                        className={styles.resume_btn}
                        disabled={!resumeText.trim() || resumingId === row.fs.id}
                        onClick={() => handleResume(row.fs)}
                      >
                        <RotateCcw size={12} strokeWidth={1.6} />
                        {resumingId === row.fs.id
                          ? t("history.resuming", "恢复中…")
                          : t("history.resume", "恢复会话")}
                      </button>
                      {resumeError && (
                        <div className={styles.resume_error}>{resumeError}</div>
                      )}
                    </div>
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
