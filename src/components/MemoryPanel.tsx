import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
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
  const [expanded, setExpanded] = useState(false);
  const [memories, setMemories] = useState<WorkspaceMemory[]>([]);
  const [loaded, setLoaded] = useState(false);

  // Detail modal state
  const [selectedFile, setSelectedFile] = useState<{
    name: string;
    path: string;
    workspaceName: string;
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
    if (expanded && !loaded) {
      load();
    }
  }, [expanded, loaded, load]);

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
                  {ws.workspaceName}
                </span>
                {ws.hasClaudeMd && (
                  <span className={`${styles.badge} ${styles.badge_claude}`}>
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
        />
      )}
    </div>
  );
}

// ── Detail Modal ─────────────────────────────────────────────────────────────

function MemoryDetailModal({
  file,
  onClose,
}: {
  file: { name: string; path: string; workspaceName: string };
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"content" | "history">("content");
  const [content, setContent] = useState<string | null>(null);
  const [history, setHistory] = useState<MemoryHistoryEntry[] | null>(null);
  const [loadingContent, setLoadingContent] = useState(false);
  const [loadingHistory, setLoadingHistory] = useState(false);

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

  return (
    <div className={styles.modal_overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div className={styles.modal_header}>
          <span className={styles.modal_title}>
            {file.workspaceName} / {file.name}
          </span>
          <button className={styles.modal_close} onClick={onClose}>
            ✕
          </button>
        </div>

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
        </div>

        {/* Body */}
        <div className={styles.modal_body}>
          {tab === "content" && (
            <>
              {loadingContent && (
                <p className={styles.loading}>{t("memory.loading")}</p>
              )}
              {content !== null && (
                <pre className={styles.content_pre}>{content}</pre>
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
      </div>
    </div>
  );
}
