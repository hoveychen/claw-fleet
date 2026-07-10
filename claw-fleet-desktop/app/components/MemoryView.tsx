import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Brain } from "lucide-react";
import { TextBlock } from "./blocks/TextBlock";
import { EmptyState } from "./EmptyState";
import { useAutoFlip } from "./useAutoFlip";
import { useUIStore } from "../store";
import { CollapsedSidebarRail } from "./CollapsedSidebarRail";
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
  const collapsed = useUIStore((s) => !!s.secondarySidebarCollapsed["memory"]);
  const setSecondarySidebar = useUIStore((s) => s.setSecondarySidebar);
  const [memories, setMemories] = useState<WorkspaceMemory[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState("");
  const [filterType, setFilterType] = useState<MemType | "all">("all");
  const [expandedUnindexed, setExpandedUnindexed] = useState<Set<string>>(new Set());
  const [selection, setSelection] = useState<Selection>(null);

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

  // Filter by query (name/title/hook/description) + by selected type.
  // MEMORY.md is always kept (it's the index).
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return memories
      .map((ws) => {
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
  }, [memories, query, filterType]);

  // Keep selection valid after refresh / filter changes.
  useEffect(() => {
    if (!selection) return;
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
    <div className={styles.page}>
      <header className={styles.header}>
        <div className={styles.title_row}>
          <h1 className={styles.title}>{t("memory.panel_title")}</h1>
          {loaded && totalFiles > 0 && (
            <span className={styles.count}>{totalFiles}</span>
          )}
        </div>
        <input
          className={styles.search}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("memory.search_placeholder")}
        />
      </header>

      <div className={styles.filter_bar}>
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
      </div>

      <div className={styles.body}>
        {collapsed ? (
          <CollapsedSidebarRail onExpand={() => setSecondarySidebar("memory", false)} />
        ) : (
        <aside className={styles.list_pane}>
          {!loaded && <p className={styles.empty}>{t("memory.loading")}</p>}
          {loaded && filtered.length === 0 && (
            <EmptyState
              icon={<Brain size={28} strokeWidth={1.5} />}
              title={t("empty_state.memory_title")}
              subtitle={t("empty_state.memory_subtitle")}
            />
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
        </aside>
        )}

        <main className={styles.detail_pane}>
          {selection === null ? (
            <div className={styles.placeholder}>
              {loaded && totalFiles > 0
                ? t("memory.panel_title")
                : t("memory.no_memories")}
            </div>
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
        </main>
      </div>
    </div>
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
          {!isIndex && (
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
        <button
          className={`${styles.tab} ${tab === "history" ? styles.tab_active : ""}`}
          onClick={() => setTab("history")}
        >
          {t("memory.tab_history")}
          {history && ` (${history.length})`}
        </button>
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
  active,
  onClick,
}: {
  file: MemoryFile;
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
      {isIndexFile ? (
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
