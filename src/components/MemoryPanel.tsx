import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./MemoryPanel.module.css";

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

// ── Helper ───────────────────────────────────────────────────────────────────

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

export function MemoryPanel() {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  const [memories, setMemories] = useState<WorkspaceMemory[]>([]);
  const [loaded, setLoaded] = useState(false);

  // Detail modal state
  const [selectedFile, setSelectedFile] = useState<{
    name: string;
    path: string;
    workspaceName: string;
    workspacePath: string;
  } | null>(null);

  // CLAUDE.md modal state
  const [claudeMdWorkspace, setClaudeMdWorkspace] = useState<{
    workspaceName: string;
    workspacePath: string;
  } | null>(null);

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
    if (!loaded) {
      load();
    }
  }, [loaded, load]);

  useEffect(() => {
    const unlisten = listen("memories-updated", () => {
      load();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [load]);

  const totalFiles = memories.reduce((sum, w) => sum + w.files.length, 0);

  return (
    <div className={styles.container}>
      <button
        className={styles.toggle}
        onClick={() => {
          setExpanded((v) => !v);
          if (!expanded && !loaded) load();
        }}
      >
        <span className={styles.toggle_label}>
          {t("memory.panel_title")}
          {loaded && totalFiles > 0 && ` (${totalFiles})`}
        </span>
        <span className={styles.toggle_icon}>{expanded ? "▲" : "▼"}</span>
      </button>

      {expanded && (
        <div className={styles.panel}>
          {!loaded && <p className={styles.empty}>{t("memory.loading")}</p>}
          {loaded && memories.length === 0 && (
            <p className={styles.empty}>{t("memory.no_memories")}</p>
          )}
          {memories.map((ws) => (
            <div key={ws.projectKey} className={styles.workspace_group}>
              <div className={styles.workspace_header}>
                <span className={styles.workspace_name}>
                  {ws.projectKey === "__global__"
                    ? t("memory.global_label")
                    : ws.workspaceName}
                </span>
                {ws.hasClaudeMd && (
                  <span
                    className={`${styles.badge} ${styles.badge_claude} ${styles.badge_clickable}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      setClaudeMdWorkspace({
                        workspaceName: ws.workspaceName,
                        workspacePath: ws.workspacePath,
                      });
                    }}
                  >
                    CLAUDE.md
                  </span>
                )}
              </div>
              <div className={styles.file_list}>
                {ws.files.map((f) => (
                  <div
                    key={f.path}
                    className={styles.file_item}
                    onClick={() =>
                      setSelectedFile({
                        name: f.name,
                        path: f.path,
                        workspaceName: ws.workspaceName,
                        workspacePath: ws.workspacePath,
                      })
                    }
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
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}

      {selectedFile && (
        <MemoryDetailModal
          file={selectedFile}
          onClose={() => setSelectedFile(null)}
          onPromoted={() => {
            setSelectedFile(null);
            load();
          }}
        />
      )}

      {claudeMdWorkspace && (
        <ClaudeMdModal
          workspaceName={claudeMdWorkspace.workspaceName}
          workspacePath={claudeMdWorkspace.workspacePath}
          onClose={() => setClaudeMdWorkspace(null)}
        />
      )}
    </div>
  );
}

// ── Detail Modal ─────────────────────────────────────────────────────────────

function MemoryDetailModal({
  file,
  onClose,
  onPromoted,
}: {
  file: { name: string; path: string; workspaceName: string; workspacePath: string };
  onClose: () => void;
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
        workspacePath: file.workspacePath,
      });
      onPromoted();
    } catch (e) {
      console.error("promote_memory failed:", e);
      setPromoting(false);
    }
  };

  const isIndex = file.name === "MEMORY.md";

  return (
    <ModalShell title={`${file.workspaceName} / ${file.name}`} onClose={onClose}>
        {/* Tabs */}
        <div className={styles.modal_tabs}>
          <button
            className={`${styles.tab} ${
              tab === "content" ? styles.tab_active : ""
            }`}
            onClick={() => setTab("content")}
          >
            {t("memory.tab_content")}
          </button>
          <button
            className={`${styles.tab} ${
              tab === "history" ? styles.tab_active : ""
            }`}
            onClick={() => setTab("history")}
          >
            {t("memory.tab_history")}
            {history && ` (${history.length})`}
          </button>

          {/* Promote button — not for MEMORY.md index */}
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

        {/* Body */}
        <div className={styles.modal_body}>
          {tab === "content" && (
            <>
              {loadingContent && (
                <p className={styles.loading}>{t("memory.loading")}</p>
              )}
              {content !== null && (
                <div className={styles.content_markdown}>
                  <TextBlock text={content} />
                </div>
              )}
            </>
          )}

          {tab === "history" && (
            <>
              {loadingHistory && (
                <p className={styles.loading}>{t("memory.loading")}</p>
              )}
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
    </ModalShell>
  );
}

// ── Modal Shell ───────────────────────────────────────────────────────────────

function ModalShell({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: React.ReactNode;
}) {
  return (
    <div className={styles.modal_overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.modal_header}>
          <span className={styles.modal_title}>{title}</span>
          <button className={styles.modal_close} onClick={onClose}>
            ✕
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

// ── CLAUDE.md Modal ───────────────────────────────────────────────────────────

function ClaudeMdModal({
  workspaceName,
  workspacePath,
  onClose,
}: {
  workspaceName: string;
  workspacePath: string;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    invoke<string>("get_claude_md_content", { workspacePath })
      .then(setContent)
      .catch(() => setContent("(error reading CLAUDE.md)"))
      .finally(() => setLoading(false));
  }, [workspacePath]);

  return (
    <ModalShell title={`${workspaceName} / CLAUDE.md`} onClose={onClose}>
      <div className={styles.modal_body}>
        {loading && <p className={styles.loading}>{t("memory.loading")}</p>}
        {content !== null && (
          <div className={styles.content_markdown}>
            <TextBlock text={content} />
          </div>
        )}
      </div>
    </ModalShell>
  );
}
