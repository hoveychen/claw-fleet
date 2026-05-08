import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
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

export interface FileBrowserSelection {
  paths: string[];
  entries: FileEntry[];
}

type ViewMode = "icon" | "list" | "column" | "gallery";
type SortKey = "name" | "modified" | "size" | "kind";
type SortDir = "asc" | "desc";

const VIEW_MODES: ViewMode[] = ["icon", "list", "column", "gallery"];

function loadPref<T extends string>(projectId: string, key: string, fallback: T): T {
  try {
    const v = window.localStorage.getItem(`fileBrowser:${projectId}:${key}`);
    return (v as T) ?? fallback;
  } catch {
    return fallback;
  }
}

function savePref(projectId: string, key: string, value: string): void {
  try {
    window.localStorage.setItem(`fileBrowser:${projectId}:${key}`, value);
  } catch {
    // localStorage unavailable — silently ignore.
  }
}

function entryKind(entry: FileEntry): string {
  if (entry.isDir) return "Folder";
  if (entry.isFleetsession) return "Fleet Session";
  const idx = entry.name.lastIndexOf(".");
  if (idx > 0) return entry.name.slice(idx + 1).toUpperCase();
  return "File";
}

function compareEntries(a: FileEntry, b: FileEntry, sortKey: SortKey, sortDir: SortDir): number {
  // Directories always come first, regardless of sort.
  if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
  let cmp = 0;
  switch (sortKey) {
    case "name":
      cmp = a.name.toLowerCase().localeCompare(b.name.toLowerCase());
      break;
    case "modified":
      cmp = a.modifiedMs - b.modifiedMs;
      break;
    case "size":
      cmp = a.sizeBytes - b.sizeBytes;
      break;
    case "kind":
      cmp = entryKind(a).localeCompare(entryKind(b));
      if (cmp === 0) cmp = a.name.toLowerCase().localeCompare(b.name.toLowerCase());
      break;
  }
  return sortDir === "asc" ? cmp : -cmp;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export function FileBrowserView({
  projectId,
  workspace,
  onSelectionChange,
}: {
  projectId: string;
  workspace: string;
  onSelectionChange?: (selection: FileBrowserSelection) => void;
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
  const [viewMode, setViewModeState] = useState<ViewMode>(() =>
    loadPref<ViewMode>(projectId, "viewMode", "icon"),
  );
  const [sortKey, setSortKeyState] = useState<SortKey>(() =>
    loadPref<SortKey>(projectId, "sortKey", "name"),
  );
  const [sortDir, setSortDirState] = useState<SortDir>(() =>
    loadPref<SortDir>(projectId, "sortDir", "asc"),
  );
  const [columnEntries, setColumnEntries] = useState<Map<string, FileEntry[]>>(new Map());

  const setViewMode = useCallback(
    (m: ViewMode) => {
      setViewModeState(m);
      savePref(projectId, "viewMode", m);
    },
    [projectId],
  );
  const toggleSort = useCallback(
    (key: SortKey) => {
      if (sortKey === key) {
        const next: SortDir = sortDir === "asc" ? "desc" : "asc";
        setSortDirState(next);
        savePref(projectId, "sortDir", next);
      } else {
        setSortKeyState(key);
        setSortDirState("asc");
        savePref(projectId, "sortKey", key);
        savePref(projectId, "sortDir", "asc");
      }
    },
    [projectId, sortKey, sortDir],
  );

  // Reset to workspace root when project changes.
  useEffect(() => {
    setCurrentDir(workspace);
    setSelected(new Set());
  }, [workspace]);

  // ── Drag & drop state ────────────────────────────────────────────────────
  const dragPathsRef = useRef<string[]>([]);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [renamingPath, setRenamingPath] = useState<string | null>(null);
  const [deleteConfirm, setDeleteConfirm] = useState<{ paths: string[]; names: string[] } | null>(null);
  const [newFolderDialog, setNewFolderDialog] = useState(false);

  const flashToast = useCallback((msg: string, ms = 2400) => {
    setToast(msg);
    window.setTimeout(() => setToast(null), ms);
  }, []);

  const sortedEntries = useMemo(
    () => [...entries].sort((a, b) => compareEntries(a, b, sortKey, sortDir)),
    [entries, sortKey, sortDir],
  );

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

  const beginDrag = useCallback(
    (e: React.DragEvent, entry: FileEntry) => {
      const paths = selected.has(entry.path) ? Array.from(selected) : [entry.path];
      dragPathsRef.current = paths;
      e.dataTransfer.effectAllowed = "copyMove";
      e.dataTransfer.setData("application/x-fleet-paths", JSON.stringify(paths));
    },
    [selected],
  );

  const isValidDrop = useCallback((targetDir: string): boolean => {
    const paths = dragPathsRef.current;
    if (paths.length === 0) return false;
    for (const src of paths) {
      if (src === targetDir) return false;
      const parent = src.replace(/\/[^/]+\/?$/, "");
      if (parent === targetDir) return false;
      if (targetDir === src || targetDir.startsWith(src + "/")) return false;
    }
    return true;
  }, []);

  const dragOverDir = useCallback(
    (e: React.DragEvent, targetDir: string) => {
      if (!isValidDrop(targetDir)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = e.altKey ? "copy" : "move";
      setDropTarget(targetDir);
    },
    [isValidDrop],
  );

  const dragLeave = useCallback(() => {
    setDropTarget(null);
  }, []);

  const dropOnDir = useCallback(
    async (e: React.DragEvent, targetDir: string) => {
      e.preventDefault();
      setDropTarget(null);
      const paths = dragPathsRef.current;
      dragPathsRef.current = [];
      if (paths.length === 0 || !isValidDrop(targetDir)) return;
      const copy = e.altKey;
      const cmd = copy ? "copy_path" : "move_path";
      let failures = 0;
      for (const src of paths) {
        const name = src.split("/").pop() ?? src;
        const dest = `${targetDir.replace(/\/$/, "")}/${name}`;
        try {
          await invoke(cmd, { from: src, to: dest });
        } catch (err) {
          failures += 1;
          flashToast(String(err));
        }
      }
      if (failures < paths.length) {
        setSelected(new Set());
        setColumnEntries(new Map());
        await refresh();
      }
    },
    [flashToast, isValidDrop, refresh],
  );

  // ── Context-menu actions ────────────────────────────────────────────────
  const selectedEntries = useMemo(
    () => entries.filter((e) => selected.has(e.path)),
    [entries, selected],
  );
  const singleSelectedEntry = selectedEntries.length === 1 ? selectedEntries[0] : null;

  const closeMenu = () => setContextMenu(null);

  const startRename = () => {
    if (!singleSelectedEntry) return;
    setRenamingPath(singleSelectedEntry.path);
    closeMenu();
  };

  const commitRename = useCallback(
    async (entry: FileEntry, newName: string) => {
      const trimmed = newName.trim();
      setRenamingPath(null);
      if (!trimmed || trimmed === entry.name) return;
      try {
        await invoke<string>("rename_path", { path: entry.path, newName: trimmed });
        setSelected(new Set());
        setColumnEntries(new Map());
        await refresh();
      } catch (e) {
        flashToast(String(e));
      }
    },
    [flashToast, refresh],
  );

  const startDelete = () => {
    if (selectedEntries.length === 0) return;
    setDeleteConfirm({
      paths: selectedEntries.map((e) => e.path),
      names: selectedEntries.map((e) => e.name),
    });
    closeMenu();
  };

  const confirmDelete = useCallback(async () => {
    if (!deleteConfirm) return;
    let failures = 0;
    for (const path of deleteConfirm.paths) {
      try {
        await invoke("delete_path", { path, toTrash: true });
      } catch (e) {
        failures += 1;
        flashToast(String(e));
      }
    }
    setDeleteConfirm(null);
    if (failures < deleteConfirm.paths.length) {
      setSelected(new Set());
      setColumnEntries(new Map());
      await refresh();
    }
  }, [deleteConfirm, flashToast, refresh]);

  const handleNewFolder = useCallback(
    async (name: string) => {
      const trimmed = name.trim();
      setNewFolderDialog(false);
      if (!trimmed) return;
      try {
        await invoke<string>("mkdir", { parent: currentDir, name: trimmed });
        setColumnEntries(new Map());
        await refresh();
      } catch (e) {
        flashToast(String(e));
      }
    },
    [currentDir, flashToast, refresh],
  );

  const handleDuplicate = useCallback(async () => {
    if (!singleSelectedEntry) return;
    closeMenu();
    try {
      await invoke<string>("duplicate_path", { path: singleSelectedEntry.path });
      setColumnEntries(new Map());
      await refresh();
    } catch (e) {
      flashToast(String(e));
    }
  }, [singleSelectedEntry, flashToast, refresh]);

  const handleCopyPath = useCallback(async () => {
    if (!singleSelectedEntry) return;
    closeMenu();
    try {
      await writeText(singleSelectedEntry.path);
      flashToast(t("file_browser.toast_path_copied"));
    } catch (e) {
      flashToast(String(e));
    }
  }, [singleSelectedEntry, flashToast, t]);

  const handleReveal = useCallback(async () => {
    if (!singleSelectedEntry) return;
    closeMenu();
    try {
      await invoke("reveal_in_system", { path: singleSelectedEntry.path });
    } catch (e) {
      flashToast(String(e));
    }
  }, [singleSelectedEntry, flashToast]);

  const handleOpen = useCallback(() => {
    if (!singleSelectedEntry) return;
    closeMenu();
    handleDoubleClick(singleSelectedEntry);
  }, [singleSelectedEntry]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Keyboard navigation ──────────────────────────────────────────────────
  const moveCursor = useCallback(
    (delta: number, extend: boolean) => {
      if (sortedEntries.length === 0) return;
      const lastSelected = Array.from(selected).pop();
      const currentIdx = lastSelected
        ? sortedEntries.findIndex((e) => e.path === lastSelected)
        : -1;
      const baseIdx = currentIdx === -1 ? 0 : currentIdx + delta;
      const nextIdx = Math.max(0, Math.min(sortedEntries.length - 1, baseIdx));
      const target = sortedEntries[nextIdx];
      if (!target) return;
      if (extend) {
        setSelected((prev) => {
          const next = new Set(prev);
          next.add(target.path);
          return next;
        });
      } else {
        setSelected(new Set([target.path]));
      }
    },
    [sortedEntries, selected],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      // Skip when a modal/input is active. Renaming inputs handle their own
      // keys and stop propagation; the dialogs are above the browser layer.
      if (renamingPath || deleteConfirm || newFolderDialog || launcherInitial) return;
      // Don't hijack typing inside descendant inputs.
      const tag = (e.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      const meta = e.metaKey || e.ctrlKey;
      const shift = e.shiftKey;
      switch (e.key) {
        case "ArrowDown":
        case "ArrowRight":
          e.preventDefault();
          moveCursor(1, shift);
          return;
        case "ArrowUp":
        case "ArrowLeft":
          e.preventDefault();
          moveCursor(-1, shift);
          return;
        case "Enter":
          if (singleSelectedEntry) {
            e.preventDefault();
            handleDoubleClick(singleSelectedEntry);
          }
          return;
        case "Backspace":
        case "Delete":
          if (meta && selectedEntries.length > 0) {
            e.preventDefault();
            startDelete();
          }
          return;
        case "a":
        case "A":
          if (meta) {
            e.preventDefault();
            setSelected(new Set(sortedEntries.map((x) => x.path)));
          }
          return;
        case "n":
        case "N":
          if (meta && shift) {
            e.preventDefault();
            setNewFolderDialog(true);
          }
          return;
        case " ":
          // Space → QuickLook on the focused entry.
          if (singleSelectedEntry) {
            e.preventDefault();
            invoke("quick_look", { path: singleSelectedEntry.path }).catch((err) =>
              flashToast(String(err)),
            );
          }
          return;
        case "F2":
          if (singleSelectedEntry) {
            e.preventDefault();
            setRenamingPath(singleSelectedEntry.path);
          }
          return;
        case "Escape":
          if (selected.size > 0) {
            e.preventDefault();
            setSelected(new Set());
          }
          return;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      renamingPath,
      deleteConfirm,
      newFolderDialog,
      launcherInitial,
      moveCursor,
      singleSelectedEntry,
      selectedEntries.length,
      sortedEntries,
      selected,
      flashToast,
    ],
  );

  useEffect(() => {
    if (!onSelectionChange) return;
    const paths = Array.from(selected);
    const entryMap = new Map(entries.map((e) => [e.path, e]));
    onSelectionChange({
      paths,
      entries: paths.map((p) => entryMap.get(p)).filter((e): e is FileEntry => Boolean(e)),
    });
  }, [selected, entries, onSelectionChange]);

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

  const parentDir = useMemo(() => {
    if (currentDir === workspace) return null;
    const parent = currentDir.replace(/\/[^/]+\/?$/, "");
    if (!parent || !parent.startsWith(workspace.replace(/\/$/, ""))) return null;
    return parent || "/";
  }, [currentDir, workspace]);

  const upDir = () => {
    if (parentDir != null) {
      setCurrentDir(parentDir);
      setSelected(new Set());
    }
  };

  // Column view derivation: each ancestor directory under workspace becomes
  // a pane, plus the current directory itself.
  const columnDirs = useMemo(() => {
    if (!currentDir.startsWith(workspace)) return [workspace];
    const rel = currentDir.slice(workspace.length).replace(/^\/+/, "");
    if (!rel) return [workspace];
    const out = [workspace];
    let acc = workspace;
    for (const p of rel.split("/")) {
      acc = acc + "/" + p;
      out.push(acc);
    }
    return out;
  }, [currentDir, workspace]);

  // Fetch column entries lazily when in column mode.
  useEffect(() => {
    if (viewMode !== "column") return;
    const missing = columnDirs.filter((d) => !columnEntries.has(d));
    if (missing.length === 0) return;
    let cancelled = false;
    (async () => {
      const next = new Map(columnEntries);
      for (const dir of missing) {
        try {
          const items = await invoke<FileEntry[]>("list_directory", { dirPath: dir });
          if (cancelled) return;
          next.set(dir, items);
        } catch {
          if (cancelled) return;
          next.set(dir, []);
        }
      }
      if (!cancelled) setColumnEntries(next);
    })();
    return () => {
      cancelled = true;
    };
  }, [viewMode, columnDirs, columnEntries]);

  // Reset column cache when we leave a project (workspace changes).
  useEffect(() => {
    setColumnEntries(new Map());
  }, [workspace]);

  const renderTile = (entry: FileEntry, key?: string) => {
    const isSel = selected.has(entry.path);
    const fleetStatus = entry.fleetSessionId
      ? fleetSessionsById.get(entry.fleetSessionId)?.status
      : undefined;
    const isDropHover = dropTarget === entry.path;
    const isRenaming = renamingPath === entry.path;
    return (
      <div
        key={key ?? entry.path}
        className={`${styles.tile} ${isSel ? styles.tile_selected : ""} ${entry.isFleetsession ? styles.tile_fleetsession : ""} ${isDropHover ? styles.tile_drop_target : ""}`}
        data-status={fleetStatus}
        draggable={!isRenaming}
        onDragStart={isRenaming ? undefined : (e) => beginDrag(e, entry)}
        onDragOver={entry.isDir ? (e) => dragOverDir(e, entry.path) : undefined}
        onDragLeave={entry.isDir ? dragLeave : undefined}
        onDrop={entry.isDir ? (e) => dropOnDir(e, entry.path) : undefined}
        onClick={isRenaming ? undefined : (e) => handleClick(entry, e)}
        onDoubleClick={isRenaming ? undefined : () => handleDoubleClick(entry)}
        onContextMenu={isRenaming ? undefined : (e) => openContextMenu(e, entry)}
        title={entry.path}
      >
        <span className={styles.tile_icon} aria-hidden>
          {entry.isFleetsession ? statusEmoji(fleetStatus) : entry.isDir ? "📁" : "📄"}
        </span>
        {isRenaming ? (
          <RenameInput entry={entry} onCommit={commitRename} onCancel={() => setRenamingPath(null)} />
        ) : (
          <span className={styles.tile_name}>{entry.name}</span>
        )}
      </div>
    );
  };

  const renderListRow = (entry: FileEntry) => {
    const isSel = selected.has(entry.path);
    const fleetStatus = entry.fleetSessionId
      ? fleetSessionsById.get(entry.fleetSessionId)?.status
      : undefined;
    const isDropHover = dropTarget === entry.path;
    return (
      <tr
        key={entry.path}
        className={`${styles.list_row} ${isSel ? styles.list_row_selected : ""} ${isDropHover ? styles.list_row_drop : ""}`}
        draggable
        onDragStart={(e) => beginDrag(e, entry)}
        onDragOver={entry.isDir ? (e) => dragOverDir(e, entry.path) : undefined}
        onDragLeave={entry.isDir ? dragLeave : undefined}
        onDrop={entry.isDir ? (e) => dropOnDir(e, entry.path) : undefined}
        onClick={(e) => handleClick(entry, e)}
        onDoubleClick={() => handleDoubleClick(entry)}
        onContextMenu={(e) => openContextMenu(e, entry)}
        title={entry.path}
      >
        <td className={styles.list_cell_name}>
          <span className={styles.list_icon} aria-hidden>
            {entry.isFleetsession ? statusEmoji(fleetStatus) : entry.isDir ? "📁" : "📄"}
          </span>
          {renamingPath === entry.path ? (
            <RenameInput entry={entry} onCommit={commitRename} onCancel={() => setRenamingPath(null)} />
          ) : (
            entry.name
          )}
        </td>
        <td className={styles.list_cell}>
          {entry.modifiedMs ? new Date(entry.modifiedMs).toLocaleString() : "—"}
        </td>
        <td className={styles.list_cell}>{entry.isDir ? "—" : formatBytes(entry.sizeBytes)}</td>
        <td className={styles.list_cell}>{entryKind(entry)}</td>
      </tr>
    );
  };

  const sortIndicator = (key: SortKey) =>
    sortKey === key ? (sortDir === "asc" ? " ▲" : " ▼") : "";

  const galleryFocus =
    selected.size === 1
      ? sortedEntries.find((e) => selected.has(e.path)) ?? null
      : null;

  return (
    <div
      ref={containerRef}
      className={styles.browser}
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onContextMenu={(e) => openContextMenu(e)}
    >
      <div className={styles.toolbar}>
        <button
          type="button"
          className={`${styles.up_btn} ${dropTarget === parentDir ? styles.up_btn_drop : ""}`}
          onClick={upDir}
          disabled={currentDir === workspace}
          onDragOver={parentDir ? (e) => dragOverDir(e, parentDir) : undefined}
          onDragLeave={parentDir ? dragLeave : undefined}
          onDrop={parentDir ? (e) => dropOnDir(e, parentDir) : undefined}
        >
          ↑
        </button>
        <div className={styles.breadcrumbs}>
          {breadcrumbs.map((b, i) => (
            <span key={b.path}>
              {i > 0 && <span className={styles.crumb_sep}>/</span>}
              <button
                type="button"
                className={`${styles.crumb} ${dropTarget === b.path ? styles.crumb_drop : ""}`}
                onClick={() => {
                  setCurrentDir(b.path);
                  setSelected(new Set());
                }}
                onDragOver={(e) => dragOverDir(e, b.path)}
                onDragLeave={dragLeave}
                onDrop={(e) => dropOnDir(e, b.path)}
              >
                {b.name}
              </button>
            </span>
          ))}
        </div>
        <div className={styles.view_segmented} role="tablist" aria-label={t("file_browser.view_mode")}>
          {VIEW_MODES.map((m) => (
            <button
              key={m}
              type="button"
              className={`${styles.view_seg_btn} ${viewMode === m ? styles.view_seg_btn_active : ""}`}
              onClick={() => setViewMode(m)}
              title={t(`file_browser.view_${m}`)}
              aria-pressed={viewMode === m}
            >
              {m === "icon" ? "▦" : m === "list" ? "☰" : m === "column" ? "▥" : "▢"}
            </button>
          ))}
        </div>
        <button type="button" className={styles.refresh_btn} onClick={refresh} title={t("file_browser.refresh")}>
          ⟳
        </button>
      </div>

      {error && <div className={styles.error}>{error}</div>}

      {viewMode === "icon" ? (
        <div className={styles.grid}>
          {sortedEntries.length === 0 && !error ? (
            <div className={styles.empty}>{t("file_browser.empty")}</div>
          ) : (
            sortedEntries.map((entry) => renderTile(entry))
          )}
        </div>
      ) : viewMode === "list" ? (
        <div className={styles.list_wrap}>
          <table className={styles.list_table}>
            <thead>
              <tr>
                <th
                  className={styles.list_header}
                  onClick={() => toggleSort("name")}
                  data-active={sortKey === "name"}
                >
                  {t("file_browser.col_name")}{sortIndicator("name")}
                </th>
                <th
                  className={styles.list_header}
                  onClick={() => toggleSort("modified")}
                  data-active={sortKey === "modified"}
                >
                  {t("file_browser.col_modified")}{sortIndicator("modified")}
                </th>
                <th
                  className={styles.list_header}
                  onClick={() => toggleSort("size")}
                  data-active={sortKey === "size"}
                >
                  {t("file_browser.col_size")}{sortIndicator("size")}
                </th>
                <th
                  className={styles.list_header}
                  onClick={() => toggleSort("kind")}
                  data-active={sortKey === "kind"}
                >
                  {t("file_browser.col_kind")}{sortIndicator("kind")}
                </th>
              </tr>
            </thead>
            <tbody>
              {sortedEntries.length === 0 && !error ? (
                <tr>
                  <td colSpan={4} className={styles.empty}>{t("file_browser.empty")}</td>
                </tr>
              ) : (
                sortedEntries.map((entry) => renderListRow(entry))
              )}
            </tbody>
          </table>
        </div>
      ) : viewMode === "column" ? (
        <div className={styles.column_wrap}>
          {columnDirs.map((dir) => {
            const items = columnEntries.get(dir);
            const sorted = items
              ? [...items].sort((a, b) => compareEntries(a, b, sortKey, sortDir))
              : null;
            return (
              <div key={dir} className={styles.column_pane}>
                {sorted == null ? (
                  <div className={styles.column_loading}>…</div>
                ) : sorted.length === 0 ? (
                  <div className={styles.empty}>{t("file_browser.empty")}</div>
                ) : (
                  sorted.map((entry) => {
                    const isSel = selected.has(entry.path);
                    const onCrumb = currentDir.startsWith(entry.path) && entry.isDir;
                    const isDropHover = dropTarget === entry.path;
                    return (
                      <div
                        key={entry.path}
                        className={`${styles.column_row} ${isSel ? styles.column_row_selected : ""} ${onCrumb ? styles.column_row_breadcrumb : ""} ${isDropHover ? styles.column_row_drop : ""}`}
                        draggable
                        onDragStart={(e) => beginDrag(e, entry)}
                        onDragOver={entry.isDir ? (e) => dragOverDir(e, entry.path) : undefined}
                        onDragLeave={entry.isDir ? dragLeave : undefined}
                        onDrop={entry.isDir ? (e) => dropOnDir(e, entry.path) : undefined}
                        onClick={(e) => handleClick(entry, e)}
                        onDoubleClick={() => handleDoubleClick(entry)}
                        onContextMenu={(e) => openContextMenu(e, entry)}
                        title={entry.path}
                      >
                        <span className={styles.column_row_icon} aria-hidden>
                          {entry.isFleetsession ? "🗂" : entry.isDir ? "📁" : "📄"}
                        </span>
                        <span className={styles.column_row_name}>{entry.name}</span>
                        {entry.isDir && <span className={styles.column_row_chev}>›</span>}
                      </div>
                    );
                  })
                )}
              </div>
            );
          })}
        </div>
      ) : (
        <div className={styles.gallery_wrap}>
          <div className={styles.gallery_focus}>
            {galleryFocus ? (
              <>
                <div className={styles.gallery_focus_icon} aria-hidden>
                  {galleryFocus.isFleetsession
                    ? statusEmoji(
                        galleryFocus.fleetSessionId
                          ? fleetSessionsById.get(galleryFocus.fleetSessionId)?.status
                          : undefined,
                      )
                    : galleryFocus.isDir
                      ? "📁"
                      : "📄"}
                </div>
                <div className={styles.gallery_focus_meta}>
                  <div className={styles.gallery_focus_name}>{galleryFocus.name}</div>
                  <div className={styles.gallery_focus_sub}>
                    {entryKind(galleryFocus)}
                    {!galleryFocus.isDir && ` · ${formatBytes(galleryFocus.sizeBytes)}`}
                  </div>
                </div>
              </>
            ) : (
              <div className={styles.gallery_focus_placeholder}>
                {t("file_browser.gallery_select_one")}
              </div>
            )}
          </div>
          <div className={styles.gallery_strip}>
            {sortedEntries.length === 0 && !error ? (
              <div className={styles.empty}>{t("file_browser.empty")}</div>
            ) : (
              sortedEntries.map((entry) => renderTile(entry, `gallery-${entry.path}`))
            )}
          </div>
        </div>
      )}

      {contextMenu && (
        <ul
          className={styles.context_menu}
          style={{ top: contextMenu.y, left: contextMenu.x }}
          onClick={(e) => e.stopPropagation()}
        >
          {singleSelectedEntry && (
            <li className={styles.context_item} onClick={handleOpen}>
              {t("file_browser.menu_open")}
            </li>
          )}
          {singleSelectedEntry && (
            <li className={styles.context_item} onClick={handleReveal}>
              {t("file_browser.menu_reveal")}
            </li>
          )}
          {singleSelectedEntry && <li className={styles.context_sep} aria-hidden />}
          {singleSelectedEntry && (
            <li className={styles.context_item} onClick={startRename}>
              {t("file_browser.menu_rename")}
            </li>
          )}
          {singleSelectedEntry && (
            <li className={styles.context_item} onClick={handleDuplicate}>
              {t("file_browser.menu_duplicate")}
            </li>
          )}
          {singleSelectedEntry && (
            <li className={styles.context_item} onClick={handleCopyPath}>
              {t("file_browser.menu_copy_path")}
            </li>
          )}
          {selectedEntries.length > 0 && (
            <li
              className={`${styles.context_item} ${styles.context_item_danger}`}
              onClick={startDelete}
            >
              {selectedEntries.length === 1
                ? t("file_browser.menu_delete")
                : t("file_browser.menu_delete_multi", { count: selectedEntries.length })}
            </li>
          )}
          {selectedEntries.length > 0 && <li className={styles.context_sep} aria-hidden />}
          <li
            className={styles.context_item}
            onClick={() => {
              setNewFolderDialog(true);
              closeMenu();
            }}
          >
            {t("file_browser.menu_new_folder")}
          </li>
          <li className={styles.context_sep} aria-hidden />
          <li className={styles.context_item} onClick={startSessionWithSelection}>
            {t("file_browser.start_session_with_selection", { count: selected.size })}
          </li>
          <li
            className={styles.context_item}
            onClick={() => {
              setLauncherInitial({ projectId, contextFiles: [], prompt: "" });
              closeMenu();
            }}
          >
            {t("file_browser.start_session_here")}
          </li>
        </ul>
      )}

      {deleteConfirm && (
        <div className={styles.modal_overlay} onClick={() => setDeleteConfirm(null)}>
          <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
            <h3>{t("file_browser.delete_confirm_title")}</h3>
            <p>
              {deleteConfirm.paths.length === 1
                ? t("file_browser.delete_confirm_one", { name: deleteConfirm.names[0] })
                : t("file_browser.delete_confirm_many", { count: deleteConfirm.paths.length })}
            </p>
            <p className={styles.modal_hint}>{t("file_browser.delete_to_trash_hint")}</p>
            <div className={styles.modal_actions}>
              <button type="button" onClick={() => setDeleteConfirm(null)}>
                {t("cancel")}
              </button>
              <button
                type="button"
                className={styles.modal_danger}
                onClick={confirmDelete}
              >
                {t("file_browser.menu_delete")}
              </button>
            </div>
          </div>
        </div>
      )}

      {newFolderDialog && (
        <NewFolderDialog
          onCancel={() => setNewFolderDialog(false)}
          onSubmit={handleNewFolder}
          t={t}
        />
      )}

      {toast && <div className={styles.toast}>{toast}</div>}

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

function RenameInput({
  entry,
  onCommit,
  onCancel,
}: {
  entry: FileEntry;
  onCommit: (entry: FileEntry, newName: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(entry.name);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.focus();
    // Mac Finder selects the basename (without the extension) on rename.
    if (entry.isDir) {
      el.select();
      return;
    }
    const idx = entry.name.lastIndexOf(".");
    if (idx > 0) el.setSelectionRange(0, idx);
    else el.select();
  }, [entry.isDir, entry.name]);

  return (
    <input
      ref={inputRef}
      type="text"
      className={styles.rename_input}
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
      onBlur={() => onCommit(entry, value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") onCommit(entry, value);
        if (e.key === "Escape") onCancel();
      }}
    />
  );
}

function NewFolderDialog({
  onCancel,
  onSubmit,
  t,
}: {
  onCancel: () => void;
  onSubmit: (name: string) => void;
  t: (k: string) => string;
}) {
  const [name, setName] = useState("");
  return (
    <div className={styles.modal_overlay} onClick={onCancel}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <h3>{t("file_browser.new_folder_title")}</h3>
        <input
          type="text"
          className={styles.modal_input}
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("file_browser.new_folder_placeholder")}
          onKeyDown={(e) => {
            if (e.key === "Enter") onSubmit(name);
            if (e.key === "Escape") onCancel();
          }}
        />
        <div className={styles.modal_actions}>
          <button type="button" onClick={onCancel}>
            {t("cancel")}
          </button>
          <button type="button" onClick={() => onSubmit(name)} disabled={!name.trim()}>
            {t("file_browser.menu_new_folder")}
          </button>
        </div>
      </div>
    </div>
  );
}
