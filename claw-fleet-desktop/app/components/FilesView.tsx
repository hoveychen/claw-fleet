import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  FileDiff,
  FolderOpen,
  FolderPlus,
  GitBranch,
  Link2,
  RefreshCw,
  Trash2,
  Upload,
} from "lucide-react";
import { EmptyState } from "./EmptyState";
import { CopyButton } from "./CopyButton";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import { ConfirmDialog } from "./ConfirmDialog";
import { DirPickerDialog } from "./DirPickerDialog";
import { isWebBuild } from "../hostEnv";
import { CloneRepoDialog } from "./CloneRepoDialog";
import { isTempWorkspacePath, repoRootPath } from "./NewSessionForm";
import {
  runningProcCounts,
  useConnectionStore,
  useProcStore,
  useSessionsStore,
  useUIStore,
  type FileNavRequest,
} from "../store";
import { ProcPanel } from "./ProcPanel";
import { ProcTerminal } from "./ProcTerminal";
import { FilePreview, FileTree } from "./ExplorerPane";
import type { ExplorerEntry, ExplorerFileContent, RevealRequest } from "./ExplorerPane";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import { PageShell } from "./PageShell";
import styles from "./MemoryView.module.css";
import skillStyles from "./SkillsView.module.css";
import fileStyles from "./FilesView.module.css";

// ── Types (mirror claw-fleet-core/src/file_explorer.rs) ─────────────────────

interface ExplorerRoot {
  label: string;
  path: string;
  branch: string | null;
  isWorktree: boolean;
}

// mirror claw-fleet-core/src/git_ops.rs
interface DirtyFile {
  path: string;
  status: string;
}

interface GitStatus {
  isGit: boolean;
  branch: string | null;
  upstream: string | null;
  /** Remote *URL* (`git@host:owner/repo.git`) — not the `origin/main` ref name
   *  `upstream` carries. Null when the repo has no matching remote. */
  remoteUrl: string | null;
  ahead: number | null;
  behind: number | null;
  dirtyCount: number;
  dirtyFiles: DirtyFile[];
}

interface GitOpResult {
  ok: boolean;
  output: string;
}

// ── Helpers ──────────────────────────────────────────────────────────────────
//
// Tree rendering and its size/markdown helpers live in ExplorerPane, shared with
// the session scratchpad. Root-picking stays here: it is the workspace
// explorer's own concern — the scratchpad has a single root and never needs it.

/** The root that owns `absPath` — longest matching prefix, so a worktree root
 *  wins over the main checkout it lives inside. */
function pickRoot(roots: ExplorerRoot[], absPath: string): ExplorerRoot | null {
  let best: ExplorerRoot | null = null;
  for (const root of roots) {
    const prefix = root.path.endsWith("/") ? root.path : `${root.path}/`;
    if (absPath.startsWith(prefix) && (!best || root.path.length > best.path.length)) {
      best = root;
    }
  }
  return best;
}

/** Last path segment, separator-agnostic. Used to name a workspace card when
 *  the path was collapsed to a repo root (or added by hand) and no session's
 *  `workspaceName` applies. */
function basename(p: string): string {
  const normalized = p.replace(/\\/g, "/").replace(/\/+$/, "");
  const slash = normalized.lastIndexOf("/");
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

/** Everything above `p`, separator preserved. Used to pre-fill the clone
 *  dialog: a new checkout almost always belongs beside the repo in hand. Empty
 *  when `p` has no parent, which lets the dialog fall back to the host's home. */
function parentDir(p: string): string {
  const trimmed = p.replace(/[/\\]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut > 0 ? trimmed.slice(0, cut) : "";
}


// ── Root view: workspace picker + explorer ──────────────────────────────────

export function FilesView() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const fetchProcs = useProcStore((s) => s.fetchProcs);
  const procs = useProcStore((s) => s.procs);
  const fileNav = useUIStore((s) => s.fileNav);
  const { selectedWorkspace: selected } = useUIStore((s) => s.mainViewState.files);
  // Directories the user added by hand or cloned. Backend state, not UI state:
  // they widen the explorer's permission gate, and a repo with no sessions is
  // only browsable because the backend recorded it. Fetched on mount so a
  // cloned repo's card is still here after a restart.
  const [extraPaths, setExtraPaths] = useState<string[]>([]);
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const setSelected = (selectedWorkspace: string | null) =>
    updateMainViewState("files", { selectedWorkspace });
  const { connection } = useConnectionStore();
  const isRemote = connection?.type === "remote";
  // The native directory dialog browses the machine the *desktop* runs on, which
  // is the wrong one under a remote connection — and in the browser build there
  // is no native dialog at all, only bytes. Both cases go through
  // `DirPickerDialog`, which lists whatever host the backend is bound to
  // (mirrors NewSessionForm).
  const needsBackendDirPicker = isRemote || isWebBuild();
  // Directories the user browsed to by hand this session. They have no sessions
  // of their own, so they'd never surface from the session-derived list — we
  // keep them here (most-recent first) and merge them in as zero-count cards.
  const [pickingDir, setPickingDir] = useState(false);
  const [cloning, setCloning] = useState(false);

  // Row context menu — anchor + subject held together, mirroring WikiView.
  const [ctxMenu, setCtxMenu] = useState<{
    ws: { path: string; name: string; count: number };
    anchor: ContextMenuAnchor;
  } | null>(null);

  // Re-fetch whenever the backend changes underfoot: local↔remote swaps point
  // at a different host, whose registered directories are its own.
  useEffect(() => {
    void invoke<string[]>("list_browse_paths")
      .then(setExtraPaths)
      .catch(() => setExtraPaths([]));
  }, [isRemote]);

  // Drop a hand-added path (zero-count card) back out of the session view.
  const removePath = async (path: string) => {
    try {
      setExtraPaths(await invoke<string[]>("remove_browse_path", { path }));
    } catch {
      return;
    }
    if (selected === path) setSelected(null);
  };

  const wsMenuItems = (ws: {
    path: string;
    name: string;
    count: number;
  }): ContextMenuItem[] => {
    const items: ContextMenuItem[] = [];
    // Opening a file manager needs the host shell. A remote workspace lives on
    // the probe, and in the browser build `reveal_path` is a no-op — a menu item
    // that silently does nothing is worse than an absent one.
    if (!isRemote && !isWebBuild()) {
      const revealKey =
        document.documentElement.getAttribute("data-platform") === "windows"
          ? "paths.reveal_in_explorer"
          : "paths.reveal_in_finder";
      items.push({
        id: "reveal",
        label: t(revealKey),
        icon: <FolderOpen size={13} strokeWidth={1.7} />,
        onSelect: () => void invoke("reveal_path", { path: ws.path }).catch(() => {}),
      });
    }
    items.push({
      id: "copy-path",
      label: t("paths.copy_path", "复制路径"),
      icon: <Copy size={13} strokeWidth={1.7} />,
      sub: ws.path,
      onSelect: () => void writeText(ws.path).catch(() => {}),
    });
    // Only hand-added dirs (zero session count, tracked in extraPaths) can be
    // removed — session-derived cards reappear on the next scan anyway.
    if (ws.count === 0 && extraPaths.includes(ws.path)) {
      items.push({
        id: "remove",
        label: t("files.remove_path", "从列表移除"),
        icon: <Trash2 size={13} strokeWidth={1.7} />,
        danger: true,
        onSelect: () => void removePath(ws.path),
      });
    }
    return items;
  };

  // A path clicked in agent prose lands here: select its workspace, then hand
  // the request down so the explorer can expand to the file itself.
  useEffect(() => {
    if (fileNav) setSelected(fileNav.workspacePath);
  }, [fileNav]);

  // Running workspace-command count per workspace → card badges.
  const runningCounts = useMemo(() => runningProcCounts(procs), [procs]);

  // Poll the workspace-proc registry while the 文件 page is open — drives
  // both the per-workspace badges and the ProcPanel lists.
  useEffect(() => {
    void fetchProcs();
    const timer = setInterval(() => void fetchProcs(), 2000);
    return () => clearInterval(timer);
  }, [fetchProcs]);

  // Distinct workspaces across all known sessions, with a session count so the
  // busiest projects surface naturally at the top. Filtering mirrors the New
  // Session launcher (see distinctWorkspaces): in-repo worktree checkouts fold
  // onto their durable repo root, and OS temp/scratchpad cwds (the per-session
  // `/private/tmp/claude-501/...` dirs) are dropped rather than listed as repos.
  const workspaces = useMemo(() => {
    const byPath = new Map<string, { path: string; name: string; count: number }>();
    for (const s of sessions) {
      if (!s.workspacePath) continue;
      const path = repoRootPath(s.workspacePath);
      if (isTempWorkspacePath(path)) continue;
      const existing = byPath.get(path);
      if (existing) existing.count += 1;
      else
        byPath.set(path, {
          path,
          // Collapsed onto a repo root → the session's own name may be a
          // worktree leaf; use the durable root's basename instead.
          name: path === s.workspacePath ? s.workspaceName : basename(path),
          count: 1,
        });
    }
    const derived = [...byPath.values()].sort(
      (a, b) => b.count - a.count || a.name.localeCompare(b.name),
    );
    // Hand-added dirs the session list doesn't already cover, newest first.
    const extras = extraPaths
      .filter((p) => !byPath.has(p))
      .map((p) => ({ path: p, name: basename(p), count: 0 }));
    return [...extras, ...derived];
  }, [sessions, extraPaths]);

  // Any selected path resolves to a browsable workspace even when it isn't a
  // card — a raw worktree path from agent prose (the list only keeps repo
  // roots) or a just-picked directory still opens in the explorer.
  const active = useMemo(() => {
    if (!selected) return null;
    return (
      workspaces.find((w) => w.path === selected) ?? {
        path: selected,
        name: basename(selected),
        count: 0,
      }
    );
  }, [workspaces, selected]);

  // Registering is what makes the directory browsable at all, so select it only
  // once the backend has accepted it — otherwise the card opens onto the very
  // "not a known session workspace" error this replaced.
  const addPath = async (path: string) => {
    try {
      setExtraPaths(await invoke<string[]>("add_browse_path", { path }));
    } catch {
      return;
    }
    setSelected(path);
  };

  // Local desktop: the native dialog is the better experience and browses the
  // right machine. Otherwise, the backend picker (DirPickerDialog).
  const browseWorkspace = async () => {
    if (needsBackendDirPicker) {
      setPickingDir(true);
      return;
    }
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked === "string") await addPath(picked);
  };

  return (
    <PageShell
      view="files"
      className={styles.mem_scope}
      title={t("files.panel_title")}
      count={workspaces.length > 0 ? workspaces.length : null}
      actions={
        <>
          <button
            className={fileStyles.add_ws_btn}
            onClick={() => setCloning(true)}
            title={t("files.clone.title")}
          >
            <GitBranch size={13} strokeWidth={1.7} />
            <span>{t("files.clone.button")}</span>
          </button>
          <button
            className={fileStyles.add_ws_btn}
            onClick={browseWorkspace}
            title={t("files.add_path")}
          >
            <FolderPlus size={13} strokeWidth={1.7} />
            <span>{t("files.add_path")}</span>
          </button>
        </>
      }
      secondary={
        <div className={styles.list_pane}>
          {workspaces.length === 0 && (
            <EmptyState
              icon={<FolderOpen size={28} strokeWidth={1.5} />}
              title={t("files.no_workspaces_title")}
              subtitle={t("files.no_workspaces_subtitle")}
            />
          )}
          <div className={styles.card_list}>
            {workspaces.map((ws) => (
              <button
                key={ws.path}
                className={`${styles.card} ${selected === ws.path ? styles.card_active : ""}`}
                onClick={() => setSelected(ws.path)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  setCtxMenu({ ws, anchor: { x: e.clientX, y: e.clientY } });
                }}
              >
                <div className={styles.card_body}>
                  <div className={styles.card_title}>{ws.name}</div>
                  <div className={styles.card_hook}>{ws.path}</div>
                </div>
                {(runningCounts.get(ws.path) ?? 0) > 0 && (
                  <span
                    className={fileStyles.ws_badge}
                    title={t("files.proc_running")}
                  >
                    {runningCounts.get(ws.path)}
                  </span>
                )}
              </button>
            ))}
          </div>
          {ctxMenu && (
            <ContextMenu
              anchor={ctxMenu.anchor}
              items={wsMenuItems(ctxMenu.ws)}
              onClose={() => setCtxMenu(null)}
            />
          )}
        </div>
      }
    >
      {active ? (
        <WorkspaceExplorer
          key={active.path}
          workspace={active.path}
          name={active.name}
          nav={fileNav?.workspacePath === active.path ? fileNav : null}
        />
      ) : (
        <div className={styles.placeholder}>{t("files.select_workspace")}</div>
      )}
      {cloning && (
        <CloneRepoDialog
          // Beside the repo in hand by default; the picker starts at the
          // backend host's home when nothing is selected.
          initialParent={selected ? parentDir(selected) : ""}
          useBackendPicker={needsBackendDirPicker}
          onDone={(dest) => {
            // The backend already registered the clone destination, but re-adding
            // is idempotent and returns the fresh list — so this both syncs the
            // cards and opens the new repo.
            void addPath(dest);
            setCloning(false);
          }}
          onCancel={() => setCloning(false)}
        />
      )}
      {pickingDir && (
        <DirPickerDialog
          initialPath={selected ?? ""}
          onPick={(path) => {
            void addPath(path);
            setPickingDir(false);
          }}
          onCancel={() => setPickingDir(false)}
        />
      )}
    </PageShell>
  );
}

// ── Explorer for one workspace ───────────────────────────────────────────────

function WorkspaceExplorer({
  workspace,
  name,
  nav,
}: {
  workspace: string;
  name: string;
  nav: FileNavRequest | null;
}) {
  const { t } = useTranslation();
  const clearFileNav = useUIStore((s) => s.clearFileNav);
  const { showIgnored, tab, activeFilePath } = useUIStore(
    (s) => s.mainViewState.files,
  );
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const setShowIgnored = (value: boolean) =>
    updateMainViewState("files", { showIgnored: value });
  const setTab = (value: "files" | "procs") =>
    updateMainViewState("files", { tab: value });
  const procs = useProcStore((s) => s.procs);
  const [roots, setRoots] = useState<ExplorerRoot[] | null>(null);
  const [rootsError, setRootsError] = useState<string | null>(null);
  const [activeRoot, setActiveRoot] = useState<ExplorerRoot | null>(null);
  const {
    width: treeWidth,
    isDragging: treeDragging,
    onMouseDown: onTreeMouseDown,
  } = useResizableWidth("files-tree-width", { min: 140, max: 480, initial: 220 });

  const [activeFile, setActiveFile] = useState<ExplorerEntry | null>(null);
  // A clicked path that no root of this workspace owns. Held next to
  // `activeFile` rather than inside it: it has no root and no relative path, so
  // it can't take part in tree selection — it only replaces the preview pane.
  const [externalPath, setExternalPath] = useState<string | null>(null);

  const procCount = useMemo(
    () => procs.filter((p) => p.workspacePath === workspace).length,
    [procs, workspace],
  );

  // A path clicked in agent prose always means "show me the file" — pull the
  // explorer back to the files tab even if the user left it on 命令.
  useEffect(() => {
    if (nav) setTab("files");
  }, [nav]);

  useEffect(() => {
    invoke<ExplorerRoot[]>("list_explorer_roots", { workspace })
      .then((r) => {
        setRoots(r);
        const savedRoot = useUIStore.getState().mainViewState.files.activeRootPath;
        const nextRoot = r.find((root) => root.path === savedRoot) ?? r[0] ?? null;
        setActiveRoot(nextRoot);
        updateMainViewState("files", {
          activeRootPath: nextRoot?.path ?? null,
          activeFilePath: nextRoot?.path === savedRoot
            ? useUIStore.getState().mainViewState.files.activeFilePath
            : null,
        });
      })
      .catch((e) => {
        setRoots([]);
        setRootsError(String(e));
      });
  }, [workspace, updateMainViewState]);

  const loadDir = useCallback(
    async (rel: string): Promise<ExplorerEntry[] | null> => {
      if (!activeRoot) return null;
      try {
        return await invoke<ExplorerEntry[]>("list_explorer_dir", {
          workspace,
          root: activeRoot.path,
          relPath: rel,
          showIgnored,
        });
      } catch {
        return null;
      }
    },
    [workspace, activeRoot, showIgnored],
  );

  // Switching root (or toggling ignored) invalidates the selection; FileTree
  // reloads itself off the new `loadDir` identity.
  useEffect(() => {
    setActiveFile(null);
  }, [activeRoot, loadDir]);

  const readFile = useCallback(
    (relPath: string) =>
      invoke<ExplorerFileContent>("read_explorer_file", {
        workspace,
        root: activeRoot?.path ?? "",
        relPath,
      }),
    [workspace, activeRoot],
  );

  // ── Reveal a path clicked in agent prose ───────────────────────────────────
  //
  // Pick the root that owns the path and reduce it to a root-relative one; the
  // tree does the actual expanding, since it is what holds the expansion state.
  // Switching roots re-runs this effect, so a path in a root we are not showing
  // resolves on the second pass.
  const [reveal, setReveal] = useState<RevealRequest | null>(null);
  useEffect(() => {
    if (!nav || !roots || !activeRoot) return;

    const owner = pickRoot(roots, nav.absPath);
    if (!owner) {
      // Outside every root of this workspace — an agent naming /tmp/report.md,
      // a file in another repo, something under ~. There is no tree node to
      // reveal, so preview the single file on its own instead of (as this used
      // to) dropping the click on the floor with no feedback at all.
      setExternalPath(nav.absPath);
      clearFileNav();
      return;
    }
    setExternalPath(null);
    if (owner.path !== activeRoot.path) {
      setActiveRoot(owner); // this effect retries once the new root is active
      updateMainViewState("files", { activeRootPath: owner.path, activeFilePath: null });
      return;
    }

    const prefix = owner.path.endsWith("/") ? owner.path : `${owner.path}/`;
    const rel = nav.absPath.slice(prefix.length).replace(/\/$/, "");
    setReveal({ relPath: rel, nonce: nav.nonce });
  }, [nav, roots, activeRoot, clearFileNav, updateMainViewState]);

  // Reveal a file the user clicked in the source-control panel. Its path is
  // already relative to the active root (git reports work-dir-relative paths,
  // and the root IS the repo work dir), so no root-picking is needed. Clicks
  // get their own strictly-decreasing (negative) nonce space so they never
  // collide with the non-negative `fileNav` nonces the effect above uses.
  const clickNonce = useRef(0);
  const revealFile = useCallback((relPath: string) => {
    setTab("files");
    clickNonce.current -= 1;
    setReveal({ relPath, nonce: clickNonce.current });
  }, []);

  // Rehydrate the selected file object from its stable root-relative path after
  // this explorer remounts. FileTree owns directory loading, so its reveal path
  // is the single source of truth for reconstructing the entry.
  //
  // `externalPath` blocks it: revealing ends in `onPick` → `selectFile`, which
  // drops the out-of-tree preview. Without the guard, an external path clicked
  // while some tree file was already selected flashed up and was immediately
  // replaced by that file. Clearing the preview re-runs this and restores the
  // selection, which is what "close" should do anyway.
  useEffect(() => {
    if (!activeRoot || !activeFilePath || nav || externalPath) return;
    clickNonce.current -= 1;
    setReveal({ relPath: activeFilePath, nonce: clickNonce.current });
  }, [activeRoot, activeFilePath, nav, externalPath, loadDir]);

  const selectRoot = (root: ExplorerRoot) => {
    setActiveRoot(root);
    updateMainViewState("files", { activeRootPath: root.path, activeFilePath: null });
  };

  const selectFile = (entry: ExplorerEntry) => {
    setActiveFile(entry);
    setExternalPath(null); // picking in the tree leaves the out-of-tree preview
    updateMainViewState("files", { activeFilePath: entry.relativePath });
  };

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          <span className={styles.detail_name}>{name}</span>
          {tab === "files" && activeFile && (
            <>
              <span className={styles.detail_sep}>·</span>
              {activeFile.relativePath}
            </>
          )}
        </div>
        <div className={styles.detail_actions}>
          {tab === "files" && activeFile && activeRoot && (
            <CopyButton text={`${activeRoot.path}/${activeFile.relativePath}`} />
          )}
          {tab === "files" && (
            <label className={fileStyles.ignored_toggle}>
              <input
                type="checkbox"
                checked={showIgnored}
                onChange={(e) => setShowIgnored(e.target.checked)}
              />
              {t("files.show_ignored")}
            </label>
          )}
        </div>
      </div>

      <div className={styles.tabs}>
        <button
          className={`${styles.tab} ${tab === "files" ? styles.tab_active : ""}`}
          onClick={() => setTab("files")}
        >
          {t("files.tab_files")}
        </button>
        <button
          className={`${styles.tab} ${tab === "procs" ? styles.tab_active : ""}`}
          onClick={() => setTab("procs")}
        >
          {t("files.proc_panel_title")}
          {procCount > 0 && ` (${procCount})`}
        </button>
      </div>

      {tab === "procs" ? (
        <ProcPanel
          workspace={workspace}
          name={name}
          renderTerminal={(p) => <ProcTerminal proc={p} />}
        />
      ) : (
        <>
          {roots && roots.length > 1 && (
            <div className={fileStyles.root_pills}>
              {roots.map((root) => (
                <button
                  key={root.path}
                  className={`${fileStyles.root_pill} ${activeRoot?.path === root.path ? fileStyles.root_pill_active : ""}`}
                  onClick={() => selectRoot(root)}
                  title={root.path}
                >
                  {root.isWorktree && <GitBranch size={11} strokeWidth={1.5} />}
                  {root.label}
                  {!root.isWorktree && root.branch && (
                    <span className={fileStyles.root_branch}>{root.branch}</span>
                  )}
                </button>
              ))}
            </div>
          )}

          {activeRoot && (
            <GitStatusBar
              key={activeRoot.path}
              workspace={workspace}
              root={activeRoot.path}
              onRevealFile={revealFile}
            />
          )}

          <div className={skillStyles.detail_split}>
            <aside className={skillStyles.tree_pane} style={{ width: treeWidth }}>
              <div className={skillStyles.tree_label}>{t("files.tree_label")}</div>
              {roots === null && <p className={skillStyles.tree_empty}>{t("files.loading")}</p>}
              {rootsError && <p className={skillStyles.tree_empty}>{rootsError}</p>}
              {activeRoot && (
                <FileTree
                  loadDir={loadDir}
                  activeFile={activeFile}
                  onPick={selectFile}
                  reveal={reveal}
                  onRevealed={clearFileNav}
                />
              )}

              <ResizeHandle active={treeDragging} onMouseDown={onTreeMouseDown} />
            </aside>

            <div className={styles.detail_body}>
              {externalPath ? (
                <ExternalFilePreview
                  path={externalPath}
                  onClose={() => setExternalPath(null)}
                />
              ) : activeFile && activeRoot ? (
                <FilePreview file={activeFile} load={readFile} />
              ) : (
                <p className={styles.empty}>{t("files.select_file")}</p>
              )}
            </div>
          </div>
        </>
      )}
    </>
  );
}

// ── Out-of-workspace file preview ────────────────────────────────────────────

/**
 * One file that belongs to no workspace, previewed on its own.
 *
 * Agents name such paths constantly — `/tmp/report.md` they just wrote, a file
 * in a sibling repo. The tree can't hold them (they have no root), so this
 * stands in for the whole tree+preview pair: a banner saying where the file
 * lives, plus the ordinary `FilePreview` fed by the ungated backend read.
 *
 * Exported because a detail-column file tab is the same thing minus the tree:
 * one absolute path, read-only. It passes its own `label`, since a file opened
 * from a session's prose is usually *inside* a workspace — the tab is just a
 * viewer, not the out-of-tree escape hatch this banner names by default.
 */
export function ExternalFilePreview({
  path,
  onClose,
  label,
}: {
  path: string;
  onClose: () => void;
  /** Banner text. Defaults to the out-of-workspace notice. */
  label?: string;
}) {
  const { t } = useTranslation();
  const { connection } = useConnectionStore();
  const isRemote = connection?.type === "remote";
  // Same reason as the workspace menu above: no host shell to reveal into.
  const canReveal = !isRemote && !isWebBuild();

  // FilePreview keys its read off `relativePath`; for an out-of-tree file the
  // absolute path IS the key, and the rest of the entry is only display data.
  const entry = useMemo<ExplorerEntry>(
    () => ({
      name: basename(path),
      relativePath: path,
      sizeBytes: 0,
      isDir: false,
      modifiedMs: 0,
      isIgnored: false,
      isSymlink: false,
    }),
    [path],
  );

  const load = useCallback(
    (absPath: string) => invoke<ExplorerFileContent>("read_external_file", { path: absPath }),
    [],
  );

  const revealKey =
    document.documentElement.getAttribute("data-platform") === "windows"
      ? "paths.reveal_in_explorer"
      : "paths.reveal_in_finder";

  return (
    <div className={fileStyles.external_wrap}>
      <div className={fileStyles.external_bar}>
        <div className={fileStyles.external_text}>
          <span className={fileStyles.external_title}>
            {label ?? t("files.external_title")}
          </span>
          <span className={fileStyles.external_path}>{path}</span>
        </div>
        <div className={fileStyles.external_actions}>
          <CopyButton text={path} />
          {/* A remote workspace's files are on the probe host, not this Mac. */}
          {canReveal && (
            <button
              className={fileStyles.external_btn}
              onClick={() => void invoke("reveal_path", { path }).catch(() => {})}
            >
              {t(revealKey)}
            </button>
          )}
          <button className={fileStyles.external_btn} onClick={onClose}>
            {t("files.external_close")}
          </button>
        </div>
      </div>
      <div className={fileStyles.external_body}>
        <FilePreview file={entry} load={load} />
      </div>
    </div>
  );
}

// ── Source-control status bar for the active root ────────────────────────────

function GitStatusBar({
  workspace,
  root,
  onRevealFile,
}: {
  workspace: string;
  root: string;
  onRevealFile: (relPath: string) => void;
}) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [busy, setBusy] = useState<null | "push" | "pull">(null);
  const [confirm, setConfirm] = useState<null | "push" | "pull">(null);
  const [opMsg, setOpMsg] = useState<{ ok: boolean; text: string } | null>(null);
  // Whether the "N 个未提交" chip is expanded into its file list.
  const [showFiles, setShowFiles] = useState(false);

  const refresh = useCallback(() => {
    invoke<GitStatus>("git_status", { workspace, root })
      .then(setStatus)
      .catch(() => setStatus(null));
  }, [workspace, root]);

  useEffect(() => {
    setOpMsg(null);
    refresh();
  }, [refresh]);

  const runOp = useCallback(
    async (op: "push" | "pull") => {
      setConfirm(null);
      setBusy(op);
      setOpMsg(null);
      try {
        const res = await invoke<GitOpResult>(op === "push" ? "git_push" : "git_pull", {
          workspace,
          root,
        });
        setOpMsg({
          ok: res.ok,
          text: res.output || (res.ok ? t("files.scm.op_ok") : t("files.scm.op_failed")),
        });
        refresh();
      } catch (e) {
        setOpMsg({ ok: false, text: String(e) });
      } finally {
        setBusy(null);
      }
    },
    [workspace, root, refresh, t],
  );

  // Nothing to show for non-git roots (plain directories).
  if (!status || !status.isGit) return null;

  const ahead = status.ahead ?? 0;
  const behind = status.behind ?? 0;
  const dirty = status.dirtyCount;
  const clean = ahead === 0 && behind === 0 && dirty === 0;

  const confirmMsg =
    confirm === "push"
      ? t("files.scm.push_confirm", {
          branch: status.branch ?? "HEAD",
          upstream: status.upstream ?? t("files.scm.default_remote"),
        })
      : t("files.scm.pull_confirm", { branch: status.branch ?? "HEAD" });

  return (
    <div className={fileStyles.scm_bar}>
      <div className={fileStyles.scm_chips}>
        {status.remoteUrl && (
          <span className={fileStyles.scm_remote} title={status.remoteUrl}>
            <Link2 size={12} strokeWidth={1.7} />
            <span className={fileStyles.scm_remote_url}>{status.remoteUrl}</span>
            <CopyButton text={status.remoteUrl} label={t("files.scm.copy_remote")} />
          </span>
        )}
        {status.upstream == null ? (
          <span className={fileStyles.scm_muted}>{t("files.scm.no_upstream")}</span>
        ) : clean ? (
          <span className={fileStyles.scm_synced}>
            <Check size={12} strokeWidth={2} />
            {t("files.scm.synced")}
          </span>
        ) : null}
        {ahead > 0 && (
          <span className={`${fileStyles.scm_chip} ${fileStyles.scm_chip_ahead}`}>
            <ArrowUp size={12} strokeWidth={2} />
            {t("files.scm.ahead", { n: ahead })}
          </span>
        )}
        {behind > 0 && (
          <span className={`${fileStyles.scm_chip} ${fileStyles.scm_chip_behind}`}>
            <ArrowDown size={12} strokeWidth={2} />
            {t("files.scm.behind", { n: behind })}
          </span>
        )}
        {dirty > 0 && (
          <button
            type="button"
            className={`${fileStyles.scm_chip} ${fileStyles.scm_chip_dirty} ${fileStyles.scm_chip_btn}`}
            onClick={() => setShowFiles((v) => !v)}
            aria-expanded={showFiles}
            title={t("files.scm.dirty_toggle")}
          >
            <FileDiff size={12} strokeWidth={2} />
            {t("files.scm.dirty", { n: dirty })}
            {showFiles ? (
              <ChevronDown size={12} strokeWidth={2} />
            ) : (
              <ChevronRight size={12} strokeWidth={2} />
            )}
          </button>
        )}
      </div>

      <div className={fileStyles.scm_spacer} />

      <div className={fileStyles.scm_actions}>
        <button
          className={fileStyles.scm_btn}
          disabled={busy !== null}
          onClick={() => {
            setOpMsg(null);
            refresh();
          }}
          title={t("files.scm.refresh")}
          aria-label={t("files.scm.refresh")}
        >
          <RefreshCw size={12} strokeWidth={1.5} />
        </button>
        <button
          className={fileStyles.scm_btn}
          disabled={busy !== null}
          onClick={() => setConfirm("pull")}
          title={t("files.scm.pull")}
        >
          <ArrowDown size={12} strokeWidth={1.5} />
          {busy === "pull" ? t("files.scm.pulling") : t("files.scm.pull")}
        </button>
        <button
          className={fileStyles.scm_btn}
          disabled={busy !== null}
          onClick={() => setConfirm("push")}
          title={t("files.scm.push")}
        >
          <Upload size={12} strokeWidth={1.5} />
          {busy === "push" ? t("files.scm.pushing") : t("files.scm.push")}
        </button>
      </div>

      {showFiles && dirty > 0 && (
        <ul className={fileStyles.scm_files}>
          {status.dirtyFiles.map((f) => (
            <li key={f.path}>
              <button
                type="button"
                className={fileStyles.scm_file}
                onClick={() => onRevealFile(f.path)}
                title={t("files.scm.reveal_file")}
              >
                <span
                  className={fileStyles.scm_file_status}
                  data-status={f.status}
                >
                  {f.status}
                </span>
                <span className={fileStyles.scm_file_path}>{f.path}</span>
              </button>
            </li>
          ))}
        </ul>
      )}

      {opMsg && (
        <div
          className={`${fileStyles.scm_msg} ${opMsg.ok ? fileStyles.scm_msg_ok : fileStyles.scm_msg_err}`}
        >
          {opMsg.text}
        </div>
      )}

      {confirm && (
        <ConfirmDialog
          message={confirmMsg}
          onConfirm={() => void runOp(confirm)}
          onCancel={() => setConfirm(null)}
        />
      )}
    </div>
  );
}
