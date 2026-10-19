import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Brain,
  CalendarClock,
  Copy,
  FileText,
  FolderGit2,
  FolderOpen,
  FolderPlus,
  GitBranch,
  SquareTerminal,
  Trash2,
} from "lucide-react";

import { runningProcCounts, useProcStore, useSessionsStore, useUIStore } from "../store";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { isWebBuild } from "../hostEnv";
import { canRevealPath } from "../canReveal";
import {
  mergeProjects,
  pathTail,
  withBrowsePaths,
  type ProjectRow,
  type RegisteredProject,
} from "../projectList";
import { repoRootPath } from "../../../shared-ts/repoPath";
import { CloneRepoDialog } from "./CloneRepoDialog";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import { DirPickerDialog } from "./DirPickerDialog";
import { EmptyState } from "./EmptyState";
import { WorkspaceExplorer } from "./FilesView";
import { PageShell } from "./PageShell";
import { Presence } from "./Presence";
import { RoutinesPanel } from "./RoutinesPanel";
import { SkeletonList, Spinner, loadingStyles } from "./loading";
import { timeAgo } from "./timeAgo";
import styles from "./ProjectsView.module.css";
import fileStyles from "./FilesView.module.css";

export type ProjectTab = "routines" | "memory" | "code";

const TABS: ProjectTab[] = ["routines", "memory", "code"];

/** `selectedPath` of the pinned "all routines" row. Never a real path: those
 *  are absolute. */
export const ALL_ROUTINES = "*routines";

/** Everything above `p`: where a clone of a sibling repo most likely goes.
 *  Empty when `p` has no parent, which lets the dialog start at home. */
function parentDir(p: string): string {
  const trimmed = p.replace(/[/\\]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut > 0 ? trimmed.slice(0, cut) : "";
}

/**
 * 项目 (Projects): one row per code directory, and for the picked one its
 * routines, memory and code. Routines and code are shown in place; memory
 * hands over to its full page (记忆, another segment of this section)
 * pre-filtered to the project, since that page owns the editor.
 */
export function ProjectsView() {
  const { t } = useTranslation();
  const { query, selectedPath, tab } = useUIStore((s) => s.mainViewState.projects);
  const update = useUIStore((s) => s.updateMainViewState);
  const set = useCallback(
    (patch: Partial<{ query: string; selectedPath: string | null; tab: ProjectTab }>) =>
      update("projects", patch),
    [update],
  );

  const sessions = useSessionsStore((s) => s.sessions);
  const chatWorkspace = useChatWorkspace();
  const [registered, setRegistered] = useState<RegisteredProject[] | null>(null);
  useEffect(() => {
    invoke<RegisteredProject[]>("list_projects")
      .then((rows) => setRegistered(rows ?? []))
      .catch(() => setRegistered([]));
  }, []);

  // Directories added or cloned by hand. Backend state: recording them is what
  // lets the explorer browse a directory no session ran in.
  const [browsePaths, setBrowsePaths] = useState<string[] | null>(null);
  useEffect(() => {
    invoke<string[]>("list_browse_paths")
      .then((rows) => setBrowsePaths(rows ?? []))
      .catch(() => setBrowsePaths([]));
  }, []);
  // A path the backend is registering / dropping right now; the list only
  // changes once it answers, so these stand in for it meanwhile.
  const [addingPath, setAddingPath] = useState<string | null>(null);
  const [removingPath, setRemovingPath] = useState<string | null>(null);
  const [cloning, setCloning] = useState(false);
  const [pickingDir, setPickingDir] = useState(false);
  const [ctxMenu, setCtxMenu] = useState<{ row: ProjectRow; anchor: ContextMenuAnchor } | null>(null);

  // Workspace commands: the row badges and the code tab's commands list.
  // SessionList keeps this list polled app-wide.
  const procs = useProcStore((s) => s.procs);
  const procCounts = useMemo(() => {
    const byProject = new Map<string, number>();
    for (const [path, n] of runningProcCounts(procs)) {
      const key = repoRootPath(path);
      byProject.set(key, (byProject.get(key) ?? 0) + n);
    }
    return byProject;
  }, [procs]);

  const projects = useMemo<ProjectRow[] | null>(
    () =>
      registered && browsePaths
        ? withBrowsePaths(mergeProjects(registered, sessions, chatWorkspace), browsePaths)
        : null,
    [registered, browsePaths, sessions, chatWorkspace],
  );

  // Select a hand-added directory only once the backend has accepted it, or
  // the code tab opens onto "not a known session workspace".
  const addPath = async (path: string) => {
    setAddingPath(path);
    try {
      setBrowsePaths(await invoke<string[]>("add_browse_path", { path }));
    } catch {
      return;
    } finally {
      setAddingPath(null);
    }
    set({ selectedPath: path, tab: "code" });
  };

  const removePath = async (path: string) => {
    setRemovingPath(path);
    try {
      setBrowsePaths(await invoke<string[]>("remove_browse_path", { path }));
    } catch {
      return;
    } finally {
      setRemovingPath(null);
    }
    if (selectedPath === path) set({ selectedPath: null });
  };

  // The native dialog browses the machine the desktop runs on; the browser
  // build has none, so it lists the backend host through DirPickerDialog.
  const needsBackendDirPicker = isWebBuild();
  const browseForPath = async () => {
    if (needsBackendDirPicker) {
      setPickingDir(true);
      return;
    }
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked === "string") await addPath(picked);
  };

  const rowMenu = (row: ProjectRow): ContextMenuItem[] => {
    const items: ContextMenuItem[] = [];
    // The browser build has no host shell to reveal into (canReveal.ts).
    if (canRevealPath()) {
      const revealKey =
        document.documentElement.getAttribute("data-platform") === "windows"
          ? "paths.reveal_in_explorer"
          : "paths.reveal_in_finder";
      items.push({
        id: "reveal",
        label: t(revealKey),
        icon: <FolderOpen size={13} strokeWidth={1.7} />,
        onSelect: () => void invoke("reveal_path", { path: row.path }).catch(() => {}),
      });
    }
    items.push({
      id: "copy-path",
      label: t("paths.copy_path", "复制路径"),
      icon: <Copy size={13} strokeWidth={1.7} />,
      sub: row.path,
      onSelect: () => void writeText(row.path).catch(() => {}),
    });
    // Rows backed by sessions or the registry would come straight back.
    if (row.handAdded) {
      items.push({
        id: "remove",
        label: t("files.remove_path", "从列表移除"),
        icon: <Trash2 size={13} strokeWidth={1.7} />,
        danger: true,
        onSelect: () => void removePath(row.path),
      });
    }
    return items;
  };
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return projects ?? [];
    return (projects ?? []).filter((p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q));
  }, [projects, query]);
  // A path clicked in agent prose can point into a directory that is not a
  // project (a temp dir, the chat workspace). It still opens, as a bare row
  // the list does not show.
  const selected = useMemo<ProjectRow | null>(() => {
    if (!projects || !selectedPath || selectedPath === ALL_ROUTINES) return null;
    return (
      projects.find((p) => p.path === selectedPath) ?? {
        path: selectedPath,
        name: selectedPath.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || selectedPath,
        lastActiveMs: 0,
        runningTasks: 0,
      }
    );
  }, [projects, selectedPath]);

  const allSelected = selectedPath === ALL_ROUTINES;
  const list = (
    <div className={styles.list}>
      {/* Routines also live in temp and chat directories, which are never a
          project; this row is the one place those show up. */}
      <button
        type="button"
        className={`${styles.row}${allSelected ? ` ${styles.row_active}` : ""}`}
        onClick={() => set({ selectedPath: ALL_ROUTINES })}
        aria-current={allSelected ? "true" : undefined}
      >
        <span className={styles.icon}>
          <CalendarClock size={15} strokeWidth={1.6} />
        </span>
        <span className={styles.body}>
          <span className={styles.title}>{t("projects.all_routines", "全部例行")}</span>
          <span className={styles.meta}>
            <span className={styles.path}>{t("projects.all_routines_meta", "跨所有项目")}</span>
          </span>
        </span>
      </button>
      {projects === null ? (
        <SkeletonList rows={8} />
      ) : shown.length === 0 ? (
        <div className={styles.none}>
          {projects.length === 0
            ? t("projects.empty", "还没有项目。在某个目录里开过任务后，它就会出现在这里。")
            : t("projects.no_match", "没有符合搜索的项目")}
        </div>
      ) : (
        shown.map((p) => (
          <button
            key={p.path}
            type="button"
            className={`${styles.row}${p.path === selectedPath ? ` ${styles.row_active}` : ""}${
              removingPath === p.path ? ` ${loadingStyles.stale}` : ""
            }`}
            onClick={() => set({ selectedPath: p.path })}
            onContextMenu={(e) => {
              e.preventDefault();
              e.stopPropagation();
              setCtxMenu({ row: p, anchor: { x: e.clientX, y: e.clientY } });
            }}
            aria-current={p.path === selectedPath ? "true" : undefined}
            aria-busy={removingPath === p.path || undefined}
            title={p.path}
          >
            <span className={styles.icon}>
              <FolderGit2 size={15} strokeWidth={1.6} />
            </span>
            <span className={styles.body}>
              <span className={styles.title}>{p.name}</span>
              <span className={styles.meta}>
                <span className={styles.path}>{pathTail(p.path)}</span>
                {p.runningTasks > 0 && (
                  <span className={styles.running}>
                    {t("projects.running", "{{count}} 进行中", { count: p.runningTasks })}
                  </span>
                )}
                {p.lastActiveMs > 0 && <span className={styles.time}>{timeAgo(p.lastActiveMs, t)}</span>}
              </span>
            </span>
            {removingPath === p.path ? (
              <Spinner size={12} label={t("loading")} />
            ) : (
              (procCounts.get(p.path) ?? 0) > 0 && (
                <span className={fileStyles.ws_badge} title={t("files.proc_running")}>
                  {procCounts.get(p.path)}
                </span>
              )
            )}
          </button>
        ))
      )}
      {addingPath && !(projects ?? []).some((p) => p.path === addingPath) && (
        <div className={`${styles.row} ${loadingStyles.stale}`} aria-busy="true">
          <span className={styles.icon}>
            <FolderGit2 size={15} strokeWidth={1.6} />
          </span>
          <span className={styles.body}>
            <span className={styles.title}>{pathTail(addingPath)}</span>
          </span>
          <Spinner size={12} label={t("loading")} />
        </div>
      )}
      <Presence when={Boolean(ctxMenu)}>
        {ctxMenu && (
          <ContextMenu anchor={ctxMenu.anchor} items={rowMenu(ctxMenu.row)} onClose={() => setCtxMenu(null)} />
        )}
      </Presence>
    </div>
  );

  return (
    <PageShell
      view="projects"
      title={t("nav_section_projects", "项目")}
      count={projects ? shown.length : null}
      countLoading={projects === null}
      search={{
        value: query,
        onChange: (v) => set({ query: v }),
        placeholder: t("projects.search", "搜索项目名 / 路径…"),
      }}
      actions={
        <>
          <button className={fileStyles.add_ws_btn} onClick={() => setCloning(true)} title={t("files.clone.title")}>
            <GitBranch size={13} strokeWidth={1.7} />
            <span>{t("files.clone.button")}</span>
          </button>
          <button className={fileStyles.add_ws_btn} onClick={browseForPath} title={t("files.add_path")}>
            <FolderPlus size={13} strokeWidth={1.7} />
            <span>{t("files.add_path")}</span>
          </button>
        </>
      }
      secondary={list}
      detailKey={selectedPath}
    >
      {allSelected ? (
        <div className={styles.detail}>
          <div className={styles.head}>
            <div className={styles.head_text}>
              <div className={styles.head_name}>{t("projects.all_routines", "全部例行")}</div>
              <div className={styles.head_sub}>
                {t("projects.all_routines_sub", "所有项目的例行，也包括临时目录和聊天目录里的。新建请进具体项目。")}
              </div>
            </div>
          </div>
          <div className={styles.pane}>
            <RoutinesPanel projectPath={null} />
          </div>
        </div>
      ) : selected ? (
        <div className={styles.detail}>
          <div className={styles.head}>
            <div className={styles.head_text}>
              <div className={styles.head_name}>{selected.name}</div>
              <div className={styles.head_path}>{selected.path}</div>
            </div>
            <div className={styles.tab_bar} role="tablist">
              {TABS.map((id) => (
                <button
                  key={id}
                  type="button"
                  role="tab"
                  aria-selected={tab === id}
                  className={`${styles.tab_btn}${tab === id ? ` ${styles.tab_active}` : ""}`}
                  onClick={() => set({ tab: id })}
                >
                  {id === "routines"
                    ? t("projects.tab_routines", "例行")
                    : id === "memory"
                      ? t("projects.tab_memory", "记忆")
                      : t("projects.tab_code", "代码")}
                </button>
              ))}
            </div>
          </div>
          <div className={styles.pane}>
            {tab === "routines" ? (
              <RoutinesPanel key={selected.path} projectPath={selected.path} />
            ) : tab === "memory" ? (
              <ProjectMemory key={selected.path} projectPath={selected.path} />
            ) : (
              <ProjectCode projectPath={selected.path} name={selected.name} />
            )}
          </div>
        </div>
      ) : (
        <EmptyState
          icon={<FolderGit2 size={28} strokeWidth={1.4} />}
          title={t("projects.pick_title", "选一个项目")}
          subtitle={t("projects.pick_sub", "看它的例行、记忆和代码。")}
        />
      )}
      <Presence when={cloning}>
        {cloning && (
          <CloneRepoDialog
            // Beside the project in hand by default.
            initialParent={selected ? parentDir(selected.path) : ""}
            useBackendPicker={needsBackendDirPicker}
            onDone={(dest) => {
              // The backend already recorded the destination; re-adding is
              // idempotent and returns the fresh list.
              void addPath(dest);
              setCloning(false);
            }}
            onCancel={() => setCloning(false)}
          />
        )}
      </Presence>
      <Presence when={pickingDir}>
        {pickingDir && (
          <DirPickerDialog
            initialPath={selected?.path ?? ""}
            onPick={async (path) => {
              await addPath(path);
              setPickingDir(false);
            }}
            onCancel={() => setPickingDir(false)}
          />
        )}
      </Presence>
    </PageShell>
  );
}

interface MemoryFile {
  name: string;
  title?: string;
  description?: string;
  modifiedMs: number;
}

interface WorkspaceMemory {
  workspacePath: string;
  projectKey: string;
  hasClaudeMd: boolean;
  files: MemoryFile[];
}

/** The project's memory files (every harness's store whose workspace folds to
 *  this repo root). A click opens the Memory page on that file. */
function ProjectMemory({ projectPath }: { projectPath: string }) {
  const { t } = useTranslation();
  const [stores, setStores] = useState<WorkspaceMemory[] | null>(null);
  useEffect(() => {
    let alive = true;
    invoke<WorkspaceMemory[]>("list_memories")
      .then((all) => {
        if (alive) setStores((all ?? []).filter((ws) => repoRootPath(ws.workspacePath) === projectPath));
      })
      .catch(() => alive && setStores([]));
    return () => {
      alive = false;
    };
  }, [projectPath]);

  const open = (projectKey: string, selectedKey: string) => {
    const s = useUIStore.getState();
    s.updateMainViewState("memory", { workspaceFilter: projectKey, selectedKey });
    s.setViewMode("memory");
  };

  if (stores === null) return <SkeletonList rows={5} />;
  const rows = stores.flatMap((ws) => [
    ...(ws.hasClaudeMd
      ? [{ key: `claudeMd:${ws.projectKey}`, projectKey: ws.projectKey, title: "CLAUDE.md", sub: "", ms: 0, md: true }]
      : []),
    ...ws.files.map((f) => ({
      key: `file:${ws.projectKey}:${f.name}`,
      projectKey: ws.projectKey,
      title: f.title || f.name,
      sub: f.description ?? "",
      ms: f.modifiedMs,
      md: false,
    })),
  ]);
  if (rows.length === 0) {
    return (
      <EmptyState
        icon={<Brain size={28} strokeWidth={1.5} />}
        title={t("projects.memory_empty", "这个项目还没有记忆")}
        subtitle={t("projects.memory_empty_sub", "agent 在这里干活时记下的要点会出现在这里。")}
      />
    );
  }
  return (
    <div className={styles.pane_list}>
      {rows.map((r) => (
        <button key={r.key} type="button" className={styles.row} onClick={() => open(r.projectKey, r.key)}>
          <span className={styles.icon}>
            {r.md ? <FileText size={15} strokeWidth={1.6} /> : <Brain size={15} strokeWidth={1.6} />}
          </span>
          <span className={styles.body}>
            <span className={styles.title}>{r.title}</span>
            {(r.sub || r.ms > 0) && (
              <span className={styles.meta}>
                {r.sub && <span className={styles.path}>{r.sub}</span>}
                {r.ms > 0 && <span className={styles.time}>{timeAgo(r.ms, t)}</span>}
              </span>
            )}
          </span>
        </button>
      ))}
    </div>
  );
}

/** The project's code: the workspace explorer itself (tree, preview, git bar,
 *  commands). A path clicked in agent prose arrives here as `fileNav`. */
function ProjectCode({ projectPath, name }: { projectPath: string; name: string }) {
  const { t } = useTranslation();
  const terminal = useUIStore((s) => s.hostFeatures.terminal);
  const fileNav = useUIStore((s) => s.fileNav);

  return (
    <WorkspaceExplorer
      key={projectPath}
      workspace={projectPath}
      name={name}
      nav={fileNav && repoRootPath(fileNav.workspacePath) === projectPath ? fileNav : null}
      hideName
      extraActions={
        terminal && (
          <button
            type="button"
            className={styles.action}
            onClick={() => useUIStore.getState().requestTerminalNav(projectPath)}
          >
            <SquareTerminal size={13} strokeWidth={1.8} />
            {t("projects.open_terminal", "打开终端")}
          </button>
        )
      }
    />
  );
}
