import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Brain, CalendarClock, FileText, FolderGit2, FolderOpen, SquareTerminal } from "lucide-react";

import { runningProcCounts, useProcStore, useSessionsStore, useUIStore } from "../store";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { mergeProjects, pathTail, type ProjectRow, type RegisteredProject } from "../projectList";
import { repoRootPath } from "../../../shared-ts/repoPath";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { RoutinesPanel } from "./RoutinesPanel";
import { SkeletonList } from "./loading";
import { timeAgo } from "./timeAgo";
import styles from "./ProjectsView.module.css";

export type ProjectTab = "routines" | "memory" | "code";

const TABS: ProjectTab[] = ["routines", "memory", "code"];

/** `selectedPath` of the pinned "all routines" row. Never a real path: those
 *  are absolute. */
export const ALL_ROUTINES = "*routines";

/**
 * 项目 (Projects): one row per code directory, and for the picked one its
 * routines, memory and code. Routines are managed in place; memory and code
 * hand over to their full pages (记忆 / 代码 / 终端, the other segments of this
 * section) pre-filtered to the project, since those pages own the editors.
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

  const projects = useMemo<ProjectRow[] | null>(
    () => (registered ? mergeProjects(registered, sessions, chatWorkspace) : null),
    [registered, sessions, chatWorkspace],
  );
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return projects ?? [];
    return (projects ?? []).filter((p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q));
  }, [projects, query]);
  const selected = useMemo(
    () => (projects ?? []).find((p) => p.path === selectedPath) ?? null,
    [projects, selectedPath],
  );

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
            className={`${styles.row}${p.path === selectedPath ? ` ${styles.row_active}` : ""}`}
            onClick={() => set({ selectedPath: p.path })}
            aria-current={p.path === selectedPath ? "true" : undefined}
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
          </button>
        ))
      )}
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
              <ProjectCode projectPath={selected.path} />
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

/** Entry points into the project's code: the file explorer and, where the host
 *  has one, a terminal — plus how many workspace commands are running in it. */
function ProjectCode({ projectPath }: { projectPath: string }) {
  const { t } = useTranslation();
  const terminal = useUIStore((s) => s.hostFeatures.terminal);
  const procs = useProcStore((s) => s.procs);
  const fetchProcs = useProcStore((s) => s.fetchProcs);
  useEffect(() => {
    void fetchProcs();
  }, [fetchProcs]);
  const running = useMemo(() => {
    let n = 0;
    for (const [path, count] of runningProcCounts(procs)) if (repoRootPath(path) === projectPath) n += count;
    return n;
  }, [procs, projectPath]);

  const browse = () => {
    const s = useUIStore.getState();
    s.updateMainViewState("files", { selectedWorkspace: projectPath });
    s.setViewMode("files");
  };

  return (
    <div className={styles.code}>
      <div className={styles.code_stat}>
        {running > 0
          ? t("projects.procs_running", "{{count}} 个命令在运行", { count: running })
          : t("projects.procs_none", "没有在运行的命令")}
      </div>
      <div className={styles.code_actions}>
        <button type="button" className={styles.action} onClick={browse}>
          <FolderOpen size={15} strokeWidth={1.8} />
          {t("projects.browse_code", "浏览代码")}
        </button>
        {terminal && (
          <button
            type="button"
            className={styles.action}
            onClick={() => useUIStore.getState().requestTerminalNav(projectPath)}
          >
            <SquareTerminal size={15} strokeWidth={1.8} />
            {t("projects.open_terminal", "打开终端")}
          </button>
        )}
      </div>
    </div>
  );
}
