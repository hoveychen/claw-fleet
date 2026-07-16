import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Brain, GraduationCap, Trash2 } from "lucide-react";
import { TextBlock } from "./blocks/TextBlock";
import { EmptyState } from "./EmptyState";
import { useAutoFlip } from "./useAutoFlip";
import { PageShell } from "./PageShell";
import { useReportStore } from "../store";
import type { ManagedLesson } from "../types";
import styles from "./MemoryView.module.css";

// ── Types ────────────────────────────────────────────────────────────────────

interface MemoryFile {
  name: string;
  path: string;
  sizeBytes: number;
  modifiedMs: number;
  title?: string;
  hook?: string;
  memType?: string;
  description?: string;
  inIndex?: boolean;
}

type MemType = "user" | "feedback" | "project" | "reference";

const TYPE_CONFIG: Record<MemType, { short: string; cssClass: string }> = {
  user: { short: "USR", cssClass: "type_user" },
  feedback: { short: "FB", cssClass: "type_feedback" },
  project: { short: "PRJ", cssClass: "type_project" },
  reference: { short: "REF", cssClass: "type_reference" },
};

const TYPE_ORDER: MemType[] = ["user", "feedback", "project", "reference"];

function isMemType(s: string | undefined): s is MemType {
  return s === "user" || s === "feedback" || s === "project" || s === "reference";
}

interface WorkspaceMemory {
  source: "claude-code" | "codex" | string;
  workspaceName: string;
  workspacePath: string;
  projectKey: string;
  hasClaudeMd: boolean;
  files: MemoryFile[];
}

interface MemoryEditDetail {
  type: "write" | "edit";
  content?: string;
  oldString?: string;
  newString?: string;
}

interface MemoryHistoryEntry {
  sessionId: string;
  workspaceName: string;
  timestamp: string;
  tool: string;
  detail: MemoryEditDetail;
}

type Selection =
  | { kind: "file"; workspace: WorkspaceMemory; file: MemoryFile }
  | { kind: "claudeMd"; workspace: WorkspaceMemory }
  | { kind: "lessons" }
  | null;

// ── Helpers ──────────────────────────────────────────────────────────────────

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  return `${(bytes / 1024).toFixed(1)}K`;
}

function relativeTime(ms: number): string {
  if (!ms) return "";
  const diff = Date.now() - ms;
  const sec = Math.floor(diff / 1000);
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day}d ago`;
  const mo = Math.floor(day / 30);
  if (mo < 12) return `${mo}mo ago`;
  return `${Math.floor(mo / 12)}y ago`;
}

function formatTime(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return iso;
  }
}

// ── Component ────────────────────────────────────────────────────────────────

export function MemoryView() {
  const { t } = useTranslation();
  const [memories, setMemories] = useState<WorkspaceMemory[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState("");
  const [filterType, setFilterType] = useState<MemType | "all">("all");
  const [sourceFilter, setSourceFilter] = useState<"all" | "claude-code" | "codex">("all");
  const [workspaceFilter, setWorkspaceFilter] = useState<string>("all");
  const [expandedUnindexed, setExpandedUnindexed] = useState<Set<string>>(new Set());
  const [selection, setSelection] = useState<Selection>(null);

  // Lessons the user added to global guidance from the daily-report card
  // (~/.claude/fleet-lessons.md), shown as a pinned entry with per-row removal.
  const { managedLessons, loadManagedLessons, removeManagedLesson } = useReportStore();
  useEffect(() => {
    loadManagedLessons();
  }, [loadManagedLessons]);

  const load = useCallback(async () => {
    try {
      const data = await invoke<WorkspaceMemory[]>("list_memories");
      setMemories(data);
      setLoaded(true);
    } catch {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    if (!loaded) load();
  }, [loaded, load]);

  useEffect(() => {
    const unlisten = listen("memories-updated", () => load());
    return () => {
      unlisten.then((f) => f());
    };
  }, [load]);

  // Type counts across all workspaces (excludes MEMORY.md itself).
  const typeCounts = useMemo(() => {
    const counts: Record<MemType | "all", number> = {
      all: 0,
      user: 0,
      feedback: 0,
      project: 0,
      reference: 0,
    };
    for (const ws of memories) {
      for (const f of ws.files) {
        if (f.name === "MEMORY.md") continue;
        counts.all++;
        if (isMemType(f.memType)) counts[f.memType]++;
      }
    }
    return counts;
  }, [memories]);

  // Distinct workspaces for the filter dropdown, stable order. Keyed by
  // projectKey (not path) so the global bucket "__global__" is a clean,
  // collision-free option; the label falls back to the localized global name.
  const workspaces = useMemo(() => {
    const seen = new Map<string, string>();
    for (const ws of memories) {
      if (!seen.has(ws.projectKey)) {
        seen.set(
          ws.projectKey,
          ws.projectKey === "__global__"
            ? t("memory.global_label")
            : ws.workspaceName,
        );
      }
    }
    return [...seen.entries()].map(([key, name]) => ({ key, name }));
  }, [memories, t]);

  const sourceCounts = useMemo(() => ({
    "claude-code": memories
      .filter((ws) => ws.source === "claude-code")
      .reduce((sum, ws) => sum + ws.files.length, 0),
    codex: memories
      .filter((ws) => ws.source === "codex")
      .reduce((sum, ws) => sum + ws.files.length, 0),
  }), [memories]);

  // Filter by query (name/title/hook/description) + by selected type +
  // by selected workspace.
  // MEMORY.md is always kept (it's the index).
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return memories
      .map((ws) => {
        if (sourceFilter !== "all" && ws.source !== sourceFilter) return null;
        if (workspaceFilter !== "all" && ws.projectKey !== workspaceFilter) {
          return null;
        }
        const wsMatch = q && ws.workspaceName.toLowerCase().includes(q);
        const files = ws.files.filter((f) => {
          if (f.name === "MEMORY.md") return filterType === "all" && !q;
          if (filterType !== "all" && f.memType !== filterType) return false;
          if (!q) return true;
          if (wsMatch) return true;
          const hay = [
            f.name,
            f.title ?? "",
            f.hook ?? "",
            f.description ?? "",
          ]
            .join(" ")
            .toLowerCase();
          return hay.includes(q);
        });
        if (files.length === 0 && !wsMatch) return null;
        return { ...ws, files } satisfies WorkspaceMemory;
      })
      .filter((x): x is WorkspaceMemory => x !== null);
  }, [memories, query, filterType, workspaceFilter, sourceFilter]);

  // Keep selection valid after refresh / filter changes.
  useEffect(() => {
    if (!selection) return;
    if (selection.kind === "lessons") return; // not tied to a workspace
    if (selection.kind === "file") {
      const stillThere = filtered
        .find((ws) => ws.projectKey === selection.workspace.projectKey)
        ?.files.find((f) => f.path === selection.file.path);
      if (!stillThere) setSelection(null);
    } else {
      const ws = filtered.find((w) => w.projectKey === selection.workspace.projectKey);
      if (!ws || !ws.hasClaudeMd) setSelection(null);
    }
  }, [filtered, selection]);

  const totalFiles = typeCounts.all;

  const toggleUnindexed = (key: string) => {
    setExpandedUnindexed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  return (
    <PageShell
      view="memory"
      className={styles.mem_scope}
      title={t("memory.panel_title")}
      count={loaded && totalFiles > 0 ? totalFiles : null}
      search={{
        value: query,
        onChange: setQuery,
        placeholder: t("memory.search_placeholder"),
      }}
      bannerCenter={
        <select
          className={styles.ws_select}
          value={workspaceFilter}
          onChange={(e) => setWorkspaceFilter(e.target.value)}
        >
          <option value="all">{t("memory.all_workspaces")}</option>
          {workspaces.map((ws) => (
            <option key={ws.key} value={ws.key}>
              {ws.name}
            </option>
          ))}
        </select>
      }
      subBar={
        <>
          <FilterChip
            label={t("memory.source_claude")}
            count={sourceCounts["claude-code"]}
            active={sourceFilter === "claude-code"}
            onClick={() => setSourceFilter((v) => v === "claude-code" ? "all" : "claude-code")}
          />
          <FilterChip
            label={t("memory.source_codex")}
            count={sourceCounts.codex}
            active={sourceFilter === "codex"}
            onClick={() => setSourceFilter((v) => v === "codex" ? "all" : "codex")}
          />
          <FilterChip
            label={t("memory.filter_all")}
            count={typeCounts.all}
            active={filterType === "all"}
            onClick={() => setFilterType("all")}
          />
          {TYPE_ORDER.map((type) => (
            <FilterChip
              key={type}
              label={t(`memory.filter_${type}`)}
              count={typeCounts[type]}
              active={filterType === type}
              typeClass={TYPE_CONFIG[type].cssClass}
              onClick={() => setFilterType(type)}
            />
          ))}
        </>
      }
      secondary={
        <div className={styles.list_pane}>
          {!loaded && <p className={styles.empty}>{t("memory.loading")}</p>}
          {loaded && filtered.length === 0 && managedLessons.length === 0 && (
            <EmptyState
              icon={<Brain size={28} strokeWidth={1.5} />}
              title={t("empty_state.memory_title")}
              subtitle={t("empty_state.memory_subtitle")}
            />
          )}
          {managedLessons.length > 0 && (
            <div className={styles.workspace_group}>
              <div className={styles.workspace_header}>
                <span className={styles.workspace_name}>
                  {t("memory.lessons_group")}
                </span>
                <span className={styles.workspace_count}>
                  {managedLessons.length}
                </span>
              </div>
              <div className={styles.card_list}>
                <button
                  className={`${styles.card} ${
                    selection?.kind === "lessons" ? styles.card_active : ""
                  }`}
                  onClick={() => setSelection({ kind: "lessons" })}
                >
                  <span className={`${styles.type_badge} ${styles.type_index}`}>
                    <GraduationCap size={13} strokeWidth={2} />
                  </span>
                  <div className={styles.card_body}>
                    <div className={styles.card_title}>
                      {t("memory.lessons_entry_title")}
                    </div>
                    <div className={styles.card_hook}>
                      {t("memory.lessons_entry_hook")}
                    </div>
                  </div>
                </button>
              </div>
            </div>
          )}
          {filtered.map((ws) => {
            const indexed = ws.files.filter(
              (f) => f.inIndex || f.name === "MEMORY.md",
            );
            const unindexed = ws.files.filter(
              (f) => !f.inIndex && f.name !== "MEMORY.md",
            );
            const expanded = expandedUnindexed.has(ws.projectKey);
            return (
              <div key={ws.projectKey} className={styles.workspace_group}>
                <div className={styles.workspace_header}>
                  <span className={styles.workspace_name}>
                    {ws.projectKey === "__global__"
                      ? t("memory.global_label")
                      : ws.workspaceName}
                  </span>
                  <span className={styles.workspace_count}>
                    {ws.files.length}
                  </span>
                  <span className={styles.source_badge}>
                    {ws.source === "codex" ? "Codex" : "Claude"}
                  </span>
                  {ws.hasClaudeMd && (
                    <button
                      className={`${styles.badge} ${styles.badge_claude} ${
                        selection?.kind === "claudeMd" &&
                        selection.workspace.projectKey === ws.projectKey
                          ? styles.badge_active
                          : ""
                      }`}
                      onClick={() => setSelection({ kind: "claudeMd", workspace: ws })}
                    >
                      CLAUDE.md
                    </button>
                  )}
                </div>
                <div className={styles.card_list}>
                  {indexed.map((f) => (
                    <MemoryCard
                      key={f.path}
                      file={f}
                      source={ws.source}
                      active={
                        selection?.kind === "file" &&
                        selection.file.path === f.path
                      }
                      onClick={() => setSelection({ kind: "file", workspace: ws, file: f })}
                    />
                  ))}
                </div>
                {unindexed.length > 0 && (
                  <div className={styles.unindexed_section}>
                    <button
                      className={styles.unindexed_header}
                      onClick={() => toggleUnindexed(ws.projectKey)}
                    >
                      <span className={styles.unindexed_caret}>
                        {expanded ? "▼" : "▶"}
                      </span>
                      <span className={styles.unindexed_label}>
                        {t("memory.unindexed_section")}
                      </span>
                      <span className={styles.unindexed_count}>
                        {unindexed.length}
                      </span>
                    </button>
                    {expanded && (
                      <div className={styles.card_list}>
                        {unindexed.map((f) => (
                          <MemoryCard
                            key={f.path}
                            file={f}
                            source={ws.source}
                            active={
                              selection?.kind === "file" &&
                              selection.file.path === f.path
                            }
                            onClick={() => setSelection({ kind: "file", workspace: ws, file: f })}
                          />
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      }
    >
      {selection === null ? (
        <div className={styles.placeholder}>
          {loaded && totalFiles > 0
            ? t("memory.panel_title")
            : t("memory.no_memories")}
        </div>
      ) : selection.kind === "lessons" ? (
        <ManagedLessonsDetail
          lessons={managedLessons}
          onRemove={removeManagedLesson}
        />
      ) : selection.kind === "claudeMd" ? (
        <ClaudeMdDetail
          workspaceName={selection.workspace.workspaceName}
          workspacePath={selection.workspace.workspacePath}
        />
      ) : (
        <FileDetail
          workspace={selection.workspace}
          file={selection.file}
          onPromoted={() => {
            setSelection(null);
            load();
          }}
        />
      )}
    </PageShell>
  );
}

// ── File detail pane ─────────────────────────────────────────────────────────

function FileDetail({
  workspace,
  file,
  onPromoted,
}: {
  workspace: WorkspaceMemory;
  file: MemoryFile;
  onPromoted: () => void;
}) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"content" | "history">("content");
  const [content, setContent] = useState<string | null>(null);
  const [history, setHistory] = useState<MemoryHistoryEntry[] | null>(null);
  const [loadingContent, setLoadingContent] = useState(false);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [promoting, setPromoting] = useState(false);
  const [showPromoteMenu, setShowPromoteMenu] = useState(false);
  const promoteMenuRef = useRef<HTMLDivElement | null>(null);
  const promoteSide = useAutoFlip(showPromoteMenu, "below", promoteMenuRef);

  useEffect(() => {
    setTab("content");
    setContent(null);
    setHistory(null);
    setShowPromoteMenu(false);
  }, [file.path]);

  useEffect(() => {
    setLoadingContent(true);
    invoke<string>("get_memory_content", { path: file.path })
      .then(setContent)
      .catch(() => setContent("(error reading file)"))
      .finally(() => setLoadingContent(false));
  }, [file.path]);

  useEffect(() => {
    if (tab === "history" && history === null) {
      setLoadingHistory(true);
      invoke<MemoryHistoryEntry[]>("get_memory_history", { path: file.path })
        .then(setHistory)
        .catch(() => setHistory([]))
        .finally(() => setLoadingHistory(false));
    }
  }, [tab, file.path, history]);

  const handlePromote = async (target: "project" | "global") => {
    setPromoting(true);
    setShowPromoteMenu(false);
    try {
      await invoke("promote_memory", {
        memoryPath: file.path,
        target,
        workspacePath: workspace.workspacePath,
      });
      onPromoted();
    } catch (e) {
      console.error("promote_memory failed:", e);
      setPromoting(false);
    }
  };

  const isIndex = file.name === "MEMORY.md";

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          {workspace.projectKey === "__global__"
            ? t("memory.global_label")
            : workspace.workspaceName}
          <span className={styles.detail_sep}>/</span>
          <span className={styles.detail_name}>{file.name}</span>
        </div>
        <div className={styles.detail_actions}>
          {!isIndex && workspace.source === "claude-code" && (
            <div className={styles.promote_wrapper}>
              <button
                className={styles.promote_btn}
                disabled={promoting}
                onClick={() => setShowPromoteMenu((v) => !v)}
              >
                {promoting ? t("memory.promoting") : t("memory.promote")}
              </button>
              {showPromoteMenu && (
                <div
                  className={`${styles.promote_menu} ${
                    promoteSide === "above" ? styles.promote_menu_above : styles.promote_menu_below
                  }`}
                  ref={promoteMenuRef}
                >
                  <button
                    className={styles.promote_menu_item}
                    onClick={() => handlePromote("project")}
                  >
                    {t("memory.promote_project")}
                  </button>
                  <button
                    className={styles.promote_menu_item}
                    onClick={() => handlePromote("global")}
                  >
                    {t("memory.promote_global")}
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      <div className={styles.tabs}>
        <button
          className={`${styles.tab} ${tab === "content" ? styles.tab_active : ""}`}
          onClick={() => setTab("content")}
        >
          {t("memory.tab_content")}
        </button>
        {workspace.source === "claude-code" && (
          <button
            className={`${styles.tab} ${tab === "history" ? styles.tab_active : ""}`}
            onClick={() => setTab("history")}
          >
            {t("memory.tab_history")}
            {history && ` (${history.length})`}
          </button>
        )}
      </div>

      <div className={styles.detail_body}>
        {tab === "content" && (
          <>
            {loadingContent && <p className={styles.loading}>{t("memory.loading")}</p>}
            {content !== null && (
              <div className={styles.content_markdown}>
                <TextBlock text={content} />
              </div>
            )}
          </>
        )}

        {tab === "history" && (
          <>
            {loadingHistory && <p className={styles.loading}>{t("memory.loading")}</p>}
            {history !== null && history.length === 0 && (
              <p className={styles.no_history}>{t("memory.no_history")}</p>
            )}
            {history && history.length > 0 && (
              <div className={styles.history_list}>
                {history.map((entry, i) => (
                  <div key={i} className={styles.history_entry}>
                    <div className={styles.history_header}>
                      <span
                        className={`${styles.history_tool} ${
                          entry.tool === "Write"
                            ? styles.history_tool_write
                            : styles.history_tool_edit
                        }`}
                      >
                        {entry.tool}
                      </span>
                      <span className={styles.history_time}>
                        {formatTime(entry.timestamp)}
                      </span>
                      <span className={styles.history_session}>
                        {entry.sessionId.slice(0, 8)}
                      </span>
                    </div>
                    <div className={styles.history_body}>
                      {entry.detail.type === "write" && entry.detail.content}
                      {entry.detail.type === "edit" && (
                        <>
                          <span className={styles.diff_old}>
                            - {entry.detail.oldString}
                          </span>
                          <span className={styles.diff_new}>
                            + {entry.detail.newString}
                          </span>
                        </>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </div>
    </>
  );
}

// ── CLAUDE.md detail pane ────────────────────────────────────────────────────

function ClaudeMdDetail({
  workspaceName,
  workspacePath,
}: {
  workspaceName: string;
  workspacePath: string;
}) {
  const { t } = useTranslation();
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    setContent(null);
    invoke<string>("get_claude_md_content", { workspacePath })
      .then(setContent)
      .catch(() => setContent("(error reading CLAUDE.md)"))
      .finally(() => setLoading(false));
  }, [workspacePath]);

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          {workspaceName}
          <span className={styles.detail_sep}>/</span>
          <span className={styles.detail_name}>CLAUDE.md</span>
        </div>
      </div>
      <div className={styles.detail_body}>
        {loading && <p className={styles.loading}>{t("memory.loading")}</p>}
        {content !== null && (
          <div className={styles.content_markdown}>
            <TextBlock text={content} />
          </div>
        )}
      </div>
    </>
  );
}

// ── Managed lessons detail pane ──────────────────────────────────────────────

function ManagedLessonsDetail({
  lessons,
  onRemove,
}: {
  lessons: ManagedLesson[];
  onRemove: (id: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [removing, setRemoving] = useState<Set<string>>(new Set());

  const handleRemove = async (id: string) => {
    setRemoving((prev) => new Set(prev).add(id));
    try {
      await onRemove(id);
    } finally {
      setRemoving((prev) => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  };

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          <span className={styles.detail_name}>{t("memory.lessons_entry_title")}</span>
        </div>
      </div>
      <div className={styles.detail_body}>
        {lessons.length === 0 ? (
          <p className={styles.no_history}>{t("memory.lessons_empty")}</p>
        ) : (
          <div className={styles.history_list}>
            {lessons.map((l) => (
              <div key={l.id} className={styles.history_entry}>
                <div className={styles.content_markdown}>
                  <TextBlock text={l.content} />
                </div>
                {l.reason && (
                  <div className={styles.card_hook}>
                    <TextBlock text={l.reason} />
                  </div>
                )}
                <div className={styles.card_meta}>
                  <span>{l.workspaceName}</span>
                  <span className={styles.card_meta_dot}>·</span>
                  <span>{l.sessionId}</span>
                </div>
                <button
                  className={styles.promote_btn}
                  disabled={removing.has(l.id)}
                  onClick={() => handleRemove(l.id)}
                >
                  <Trash2 size={13} strokeWidth={2} />
                  {removing.has(l.id) ? t("memory.lessons_removing") : t("memory.lessons_remove")}
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}

// ── Filter chip ──────────────────────────────────────────────────────────────

function FilterChip({
  label,
  count,
  active,
  typeClass,
  onClick,
}: {
  label: string;
  count: number;
  active: boolean;
  typeClass?: string;
  onClick: () => void;
}) {
  return (
    <button
      className={`${styles.chip} ${active ? styles.chip_active : ""} ${
        typeClass ? styles[typeClass] ?? "" : ""
      }`}
      onClick={onClick}
    >
      <span className={styles.chip_label}>{label}</span>
      <span className={styles.chip_count}>{count}</span>
    </button>
  );
}

// ── Memory card (single entry in the list) ───────────────────────────────────

function MemoryCard({
  file,
  source,
  active,
  onClick,
}: {
  file: MemoryFile;
  source: string;
  active: boolean;
  onClick: () => void;
}) {
  const isIndexFile = file.name === "MEMORY.md";
  const memType = isMemType(file.memType) ? file.memType : null;
  const rawTitle = file.title ?? file.name;
  // MEMORY.md uses filename-as-title when not curated; strip the .md suffix
  // so cards don't show e.g. "feedback_ddb_fp_artifacts.md" as the title.
  const title = rawTitle === file.name ? rawTitle.replace(/\.md$/, "") : rawTitle;
  const subtitle = file.hook ?? file.description ?? "";
  return (
    <button
      className={`${styles.card} ${active ? styles.card_active : ""} ${
        isIndexFile ? styles.card_index : ""
      }`}
      onClick={onClick}
    >
      {source === "codex" ? (
        <span className={`${styles.type_badge} ${styles.type_codex}`}>CX</span>
      ) : isIndexFile ? (
        <span className={`${styles.type_badge} ${styles.type_index}`}>IDX</span>
      ) : memType ? (
        <span
          className={`${styles.type_badge} ${styles[TYPE_CONFIG[memType].cssClass]}`}
        >
          {TYPE_CONFIG[memType].short}
        </span>
      ) : (
        <span className={`${styles.type_badge} ${styles.type_unknown}`}>?</span>
      )}
      <div className={styles.card_body}>
        <div className={styles.card_title}>{title}</div>
        {subtitle && <div className={styles.card_hook}>{subtitle}</div>}
        <div className={styles.card_meta}>
          <span>{formatBytes(file.sizeBytes)}</span>
          {file.modifiedMs > 0 && (
            <>
              <span className={styles.card_meta_dot}>·</span>
              <span>{relativeTime(file.modifiedMs)}</span>
            </>
          )}
        </div>
      </div>
    </button>
  );
}
