import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFleetManagedStore } from "../store";
import { SessionLauncher, type SessionLauncherProps } from "./SessionLauncher";
import styles from "./FileBrowserView.module.css";

interface FileEntry {
  name: string;
  path: string;
  isDir: boolean;
  sizeBytes: number;
  modifiedMs: number;
  isFleetsession: boolean;
  fleetSessionId: string | null;
}

interface FleetSession {
  id: string;
  status: string;
}

export function FileBrowserView({
  projectId,
  workspace,
}: {
  projectId: string;
  workspace: string;
}) {
  const { t } = useTranslation();
  const [currentDir, setCurrentDir] = useState(workspace);
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [fleetSessionsById, setFleetSessionsById] = useState<Map<string, FleetSession>>(new Map());
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null);
  const [launcherInitial, setLauncherInitial] = useState<SessionLauncherProps["initial"]>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);

  // Reset to workspace root when project changes.
  useEffect(() => {
    setCurrentDir(workspace);
    setSelected(new Set());
  }, [workspace]);

  const refresh = useCallback(async () => {
    try {
      const items = await invoke<FileEntry[]>("list_directory", { dirPath: currentDir });
      setEntries(items);
      setError(null);
    } catch (e) {
      setEntries([]);
      setError(String(e));
    }
    // Also refresh fleet sessions so .fleetsession files can pick up status colour.
    try {
      const sessions = await invoke<FleetSession[]>("list_fleet_sessions");
      const map = new Map<string, FleetSession>();
      for (const s of sessions) map.set(s.id, s);
      setFleetSessionsById(map);
    } catch {
      // non-fatal
    }
  }, [currentDir]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const breadcrumbs = useMemo(() => {
    if (!currentDir.startsWith(workspace)) return [];
    const rel = currentDir.slice(workspace.length).replace(/^\/+/, "");
    const parts = rel ? rel.split("/") : [];
    const out: { name: string; path: string }[] = [
      { name: workspace.split("/").filter(Boolean).pop() ?? "/", path: workspace },
    ];
    let acc = workspace;
    for (const p of parts) {
      acc = acc + "/" + p;
      out.push({ name: p, path: acc });
    }
    return out;
  }, [currentDir, workspace]);

  const handleClick = (entry: FileEntry, e: React.MouseEvent) => {
    if (e.metaKey || e.ctrlKey) {
      const next = new Set(selected);
      if (next.has(entry.path)) next.delete(entry.path);
      else next.add(entry.path);
      setSelected(next);
    } else {
      setSelected(new Set([entry.path]));
    }
  };

  const handleDoubleClick = async (entry: FileEntry) => {
    if (entry.isDir) {
      setCurrentDir(entry.path);
      setSelected(new Set());
    } else if (entry.isFleetsession && entry.fleetSessionId) {
      // For now, just refresh the badge cache. Future: open SessionDetail.
      useFleetManagedStore.getState().refresh();
    } else {
      try {
        await invoke("open_path_in_system", { path: entry.path });
      } catch (e) {
        setError(String(e));
      }
    }
  };

  const openContextMenu = (e: React.MouseEvent, entry?: FileEntry) => {
    e.preventDefault();
    if (entry && !selected.has(entry.path)) {
      setSelected(new Set([entry.path]));
    }
    setContextMenu({ x: e.clientX, y: e.clientY });
  };

  // Close context menu when clicking elsewhere or pressing Escape.
  useEffect(() => {
    if (!contextMenu) return;
    const close = () => setContextMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("click", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [contextMenu]);

  const startSessionWithSelection = () => {
    const files = Array.from(selected).filter(
      (p) => !entries.find((e) => e.path === p)?.isFleetsession,
    );
    setLauncherInitial({
      projectId,
      contextFiles: files,
      prompt: files.length > 0 ? buildDefaultPromptForFiles(files) : "",
    });
    setContextMenu(null);
  };

  const upDir = () => {
    if (currentDir === workspace) return;
    const parent = currentDir.replace(/\/[^/]+\/?$/, "");
    if (parent && parent.startsWith(workspace.replace(/\/$/, ""))) {
      setCurrentDir(parent || "/");
      setSelected(new Set());
    }
  };

  return (
    <div ref={containerRef} className={styles.browser} onContextMenu={(e) => openContextMenu(e)}>
      <div className={styles.toolbar}>
        <button type="button" className={styles.up_btn} onClick={upDir} disabled={currentDir === workspace}>
          ↑
        </button>
        <div className={styles.breadcrumbs}>
          {breadcrumbs.map((b, i) => (
            <span key={b.path}>
              {i > 0 && <span className={styles.crumb_sep}>/</span>}
              <button
                type="button"
                className={styles.crumb}
                onClick={() => {
                  setCurrentDir(b.path);
                  setSelected(new Set());
                }}
              >
                {b.name}
              </button>
            </span>
          ))}
        </div>
        <button type="button" className={styles.refresh_btn} onClick={refresh} title={t("file_browser.refresh")}>
          ⟳
        </button>
      </div>

      {error && <div className={styles.error}>{error}</div>}

      <div className={styles.grid}>
        {entries.length === 0 && !error ? (
          <div className={styles.empty}>{t("file_browser.empty")}</div>
        ) : (
          entries.map((entry) => {
            const isSel = selected.has(entry.path);
            const fleetStatus = entry.fleetSessionId ? fleetSessionsById.get(entry.fleetSessionId)?.status : undefined;
            return (
              <div
                key={entry.path}
                className={`${styles.tile} ${isSel ? styles.tile_selected : ""} ${entry.isFleetsession ? styles.tile_fleetsession : ""}`}
                data-status={fleetStatus}
                onClick={(e) => handleClick(entry, e)}
                onDoubleClick={() => handleDoubleClick(entry)}
                onContextMenu={(e) => openContextMenu(e, entry)}
                title={entry.path}
              >
                <span className={styles.tile_icon} aria-hidden>
                  {entry.isFleetsession
                    ? statusEmoji(fleetStatus)
                    : entry.isDir
                      ? "📁"
                      : "📄"}
                </span>
                <span className={styles.tile_name}>{entry.name}</span>
              </div>
            );
          })
        )}
      </div>

      {contextMenu && (
        <ul
          className={styles.context_menu}
          style={{ top: contextMenu.y, left: contextMenu.x }}
          onClick={(e) => e.stopPropagation()}
        >
          <li
            className={styles.context_item}
            onClick={() => {
              startSessionWithSelection();
            }}
          >
            {t("file_browser.start_session_with_selection", { count: selected.size })}
          </li>
          <li
            className={styles.context_item}
            onClick={() => {
              setLauncherInitial({ projectId, contextFiles: [], prompt: "" });
              setContextMenu(null);
            }}
          >
            {t("file_browser.start_session_here")}
          </li>
        </ul>
      )}

      <SessionLauncher
        initial={
          launcherInitial
            ? { ...launcherInitial }
            : null
        }
        onClose={() => setLauncherInitial(null)}
        onSubmitted={() => {
          // Refresh the file grid so the new .fleetsession appears.
          refresh();
        }}
      />
    </div>
  );
}

function statusEmoji(status: string | undefined): string {
  switch (status) {
    case "queued":
      return "🕒";
    case "running":
      return "▶️";
    case "pending":
      return "⏸";
    case "complete":
      return "✅";
    default:
      return "🗂";
  }
}

function buildDefaultPromptForFiles(files: string[]): string {
  const list = files.map((f) => `- ${f}`).join("\n");
  return `Please review and work on these files:\n${list}\n\n`;
}
