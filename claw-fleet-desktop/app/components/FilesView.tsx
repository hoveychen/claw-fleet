import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowDown,
  ArrowUp,
  Check,
  FileDiff,
  FolderOpen,
  FolderPlus,
  GitBranch,
  RefreshCw,
  Upload,
} from "lucide-react";
import { EmptyState } from "./EmptyState";
import { CopyButton } from "./CopyButton";
import { ConfirmDialog } from "./ConfirmDialog";
import { DirPickerDialog } from "./DirPickerDialog";
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
interface GitStatus {
  isGit: boolean;
  branch: string | null;
  upstream: string | null;
  ahead: number | null;
  behind: number | null;
  dirtyCount: number;
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


// ── Root view: workspace picker + explorer ──────────────────────────────────

export function FilesView() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const fetchProcs = useProcStore((s) => s.fetchProcs);
  const procs = useProcStore((s) => s.procs);
  const fileNav = useUIStore((s) => s.fileNav);
  const { connection } = useConnectionStore();
  const isRemote = connection?.type === "remote";
  const [selected, setSelected] = useState<string | null>(null);
  // Directories the user browsed to by hand this session. They have no sessions
  // of their own, so they'd never surface from the session-derived list — we
  // keep them here (most-recent first) and merge them in as zero-count cards.
  const [extraPaths, setExtraPaths] = useState<string[]>([]);
  // Remote-only: the backend-driven directory picker, since the native dialog
  // would browse *this* desktop rather than the probe (mirrors NewSessionForm).
  const [pickingDir, setPickingDir] = useState(false);

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

  const addPath = (path: string) => {
    setExtraPaths((prev) => (prev.includes(path) ? prev : [path, ...prev]));
    setSelected(path);
  };

  // Local: the native dialog is the better experience and browses the right
  // machine. Remote: go through the backend picker instead (DirPickerDialog).
  const browseWorkspace = async () => {
    if (isRemote) {
      setPickingDir(true);
      return;
    }
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked === "string") addPath(picked);
  };

  return (
    <PageShell
      view="files"
      className={styles.mem_scope}
      title={t("files.panel_title")}
      count={workspaces.length > 0 ? workspaces.length : null}
      actions={
        <button
          className={fileStyles.add_ws_btn}
          onClick={browseWorkspace}
          title={t("files.add_path")}
        >
          <FolderPlus size={13} strokeWidth={1.7} />
          <span>{t("files.add_path")}</span>
        </button>
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
      {pickingDir && (
        <DirPickerDialog
          initialPath={selected ?? ""}
          onPick={(path) => {
            addPath(path);
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
  const procs = useProcStore((s) => s.procs);
  const [roots, setRoots] = useState<ExplorerRoot[] | null>(null);
  const [rootsError, setRootsError] = useState<string | null>(null);
  const [activeRoot, setActiveRoot] = useState<ExplorerRoot | null>(null);
  const [showIgnored, setShowIgnored] = useState(false);
  const [tab, setTab] = useState<"files" | "procs">("files");
  const {
    width: treeWidth,
    isDragging: treeDragging,
    onMouseDown: onTreeMouseDown,
  } = useResizableWidth("files-tree-width", { min: 140, max: 480, initial: 220 });

  const [activeFile, setActiveFile] = useState<ExplorerEntry | null>(null);

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
        setActiveRoot(r[0] ?? null);
      })
      .catch((e) => {
        setRoots([]);
        setRootsError(String(e));
      });
  }, [workspace]);

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
      // Path lies outside every root of this workspace — nothing to reveal.
      clearFileNav();
      return;
    }
    if (owner.path !== activeRoot.path) {
      setActiveRoot(owner); // this effect retries once the new root is active
      return;
    }

    const prefix = owner.path.endsWith("/") ? owner.path : `${owner.path}/`;
    const rel = nav.absPath.slice(prefix.length).replace(/\/$/, "");
    setReveal({ relPath: rel, nonce: nav.nonce });
  }, [nav, roots, activeRoot, clearFileNav]);

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
                  onClick={() => setActiveRoot(root)}
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
            <GitStatusBar key={activeRoot.path} workspace={workspace} root={activeRoot.path} />
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
                  onPick={setActiveFile}
                  reveal={reveal}
                  onRevealed={clearFileNav}
                />
              )}

              <ResizeHandle active={treeDragging} onMouseDown={onTreeMouseDown} />
            </aside>

            <div className={styles.detail_body}>
              {activeFile && activeRoot ? (
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

// ── Source-control status bar for the active root ────────────────────────────

function GitStatusBar({ workspace, root }: { workspace: string; root: string }) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [busy, setBusy] = useState<null | "push" | "pull">(null);
  const [confirm, setConfirm] = useState<null | "push" | "pull">(null);
  const [opMsg, setOpMsg] = useState<{ ok: boolean; text: string } | null>(null);

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
          <span className={`${fileStyles.scm_chip} ${fileStyles.scm_chip_dirty}`}>
            <FileDiff size={12} strokeWidth={2} />
            {t("files.scm.dirty", { n: dirty })}
          </span>
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
