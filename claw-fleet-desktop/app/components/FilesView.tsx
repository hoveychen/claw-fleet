import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowDown,
  ArrowUp,
  Check,
  FileDiff,
  FolderOpen,
  GitBranch,
  RefreshCw,
  Upload,
} from "lucide-react";
import { EmptyState } from "./EmptyState";
import { CopyButton } from "./CopyButton";
import { ConfirmDialog } from "./ConfirmDialog";
import { runningProcCounts, useProcStore, useSessionsStore, useUIStore } from "../store";
import { CollapsedSidebarRail } from "./CollapsedSidebarRail";
import { ProcPanel } from "./ProcPanel";
import { ProcTerminal } from "./ProcTerminal";
import { FilePreview, FileTree } from "./ExplorerPane";
import type { ExplorerEntry, ExplorerFileContent } from "./ExplorerPane";
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

// ── Root view: workspace picker + explorer ──────────────────────────────────

export function FilesView() {
  const { t } = useTranslation();
  const collapsed = useUIStore((s) => !!s.secondarySidebarCollapsed["files"]);
  const setSecondarySidebar = useUIStore((s) => s.setSecondarySidebar);
  const sessions = useSessionsStore((s) => s.sessions);
  const fetchProcs = useProcStore((s) => s.fetchProcs);
  const procs = useProcStore((s) => s.procs);
  const [selected, setSelected] = useState<string | null>(null);

  // Running workspace-command count per workspace → card badges.
  const runningCounts = useMemo(() => runningProcCounts(procs), [procs]);

  // Poll the workspace-proc registry while the 文件 page is open — drives
  // both the per-workspace badges and the ProcPanel lists.
  useEffect(() => {
    void fetchProcs();
    const timer = setInterval(() => void fetchProcs(), 2000);
    return () => clearInterval(timer);
  }, [fetchProcs]);

  // Distinct workspaces across all known sessions, with a session count so
  // the busiest projects surface naturally at the top.
  const workspaces = useMemo(() => {
    const byPath = new Map<string, { path: string; name: string; count: number }>();
    for (const s of sessions) {
      if (!s.workspacePath) continue;
      const existing = byPath.get(s.workspacePath);
      if (existing) existing.count += 1;
      else byPath.set(s.workspacePath, { path: s.workspacePath, name: s.workspaceName, count: 1 });
    }
    return [...byPath.values()].sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
  }, [sessions]);

  const active = workspaces.find((w) => w.path === selected) ?? null;

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <div className={styles.title_row}>
          <h1 className={styles.title}>{t("files.panel_title")}</h1>
          {workspaces.length > 0 && <span className={styles.count}>{workspaces.length}</span>}
        </div>
      </header>

      <div className={styles.body}>
        {collapsed ? (
          <CollapsedSidebarRail onExpand={() => setSecondarySidebar("files", false)} />
        ) : (
        <aside className={styles.list_pane}>
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
        </aside>
        )}

        <main className={styles.detail_pane}>
          {active ? (
            <WorkspaceExplorer key={active.path} workspace={active.path} name={active.name} />
          ) : (
            <div className={styles.placeholder}>{t("files.select_workspace")}</div>
          )}
        </main>
      </div>
    </div>
  );
}

// ── Explorer for one workspace ───────────────────────────────────────────────

function WorkspaceExplorer({ workspace, name }: { workspace: string; name: string }) {
  const { t } = useTranslation();
  const [roots, setRoots] = useState<ExplorerRoot[] | null>(null);
  const [rootsError, setRootsError] = useState<string | null>(null);
  const [activeRoot, setActiveRoot] = useState<ExplorerRoot | null>(null);
  const [showIgnored, setShowIgnored] = useState(false);

  const [activeFile, setActiveFile] = useState<ExplorerEntry | null>(null);

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

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          <span className={styles.detail_name}>{name}</span>
          {activeFile && (
            <>
              <span className={styles.detail_sep}>·</span>
              {activeFile.relativePath}
            </>
          )}
        </div>
        <div className={styles.detail_actions}>
          {activeFile && activeRoot && (
            <CopyButton text={`${activeRoot.path}/${activeFile.relativePath}`} />
          )}
          <label className={fileStyles.ignored_toggle}>
            <input
              type="checkbox"
              checked={showIgnored}
              onChange={(e) => setShowIgnored(e.target.checked)}
            />
            {t("files.show_ignored")}
          </label>
        </div>
      </div>

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
        <aside className={skillStyles.tree_pane}>
          <div className={skillStyles.tree_label}>{t("files.tree_label")}</div>
          {roots === null && <p className={skillStyles.tree_empty}>{t("files.loading")}</p>}
          {rootsError && <p className={skillStyles.tree_empty}>{rootsError}</p>}
          {activeRoot && (
            <FileTree loadDir={loadDir} activeFile={activeFile} onPick={setActiveFile} />
          )}
        </aside>

        <div className={styles.detail_body}>
          {activeFile && activeRoot ? (
            <FilePreview file={activeFile} load={readFile} />
          ) : (
            <p className={styles.empty}>{t("files.select_file")}</p>
          )}
        </div>
      </div>

      <ProcPanel
        workspace={workspace}
        name={name}
        renderTerminal={(p) => <ProcTerminal proc={p} />}
      />
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
