import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./MemoryView.module.css";

// ── Types ────────────────────────────────────────────────────────────────────

interface MemoryFile {
  name: string;
  path: string;
  sizeBytes: number;
  modifiedMs: number;
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

  // Filter by query — matches workspace name or file name.
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return memories;
    return memories
      .map((ws) => {
        const wsMatch = ws.workspaceName.toLowerCase().includes(q);
        const files = wsMatch ? ws.files : ws.files.filter((f) => f.name.toLowerCase().includes(q));
        if (!wsMatch && files.length === 0) return null;
        return { ...ws, files } satisfies WorkspaceMemory;
      })
      .filter((x): x is WorkspaceMemory => x !== null);
  }, [memories, query]);

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

  const totalFiles = memories.reduce((sum, w) => sum + w.files.length, 0);

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
          placeholder={t("memory.panel_title")}
        />
      </header>

      <div className={styles.body}>
        <aside className={styles.list_pane}>
          {!loaded && <p className={styles.empty}>{t("memory.loading")}</p>}
          {loaded && filtered.length === 0 && (
            <p className={styles.empty}>{t("memory.no_memories")}</p>
          )}
          {filtered.map((ws) => (
            <div key={ws.projectKey} className={styles.workspace_group}>
              <div className={styles.workspace_header}>
                <span className={styles.workspace_name}>
                  {ws.projectKey === "__global__"
                    ? t("memory.global_label")
                    : ws.workspaceName}
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
              <div className={styles.file_list}>
                {ws.files.map((f) => {
                  const active =
                    selection?.kind === "file" &&
                    selection.file.path === f.path;
                  return (
                    <button
                      key={f.path}
                      className={`${styles.file_item} ${active ? styles.file_item_active : ""}`}
                      onClick={() => setSelection({ kind: "file", workspace: ws, file: f })}
                    >
                      <span className={styles.file_icon}>📄</span>
                      <span
                        className={`${styles.file_name} ${
                          f.name === "MEMORY.md" ? styles.file_name_index : ""
                        }`}
                      >
                        {f.name}
                      </span>
                      <span className={styles.file_size}>
                        {formatBytes(f.sizeBytes)}
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </aside>

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
                <div className={styles.promote_menu}>
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
