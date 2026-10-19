import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { BookOpen, Brain, CalendarClock, ChevronRight, FileText, Plus, SquareTerminal } from "lucide-react";

import { useDecisionStore, useProcStore, useSessionsStore, useUIStore } from "../store";
import { isQuietAliveSticky, isWakingSticky } from "../types";
import { useWikiDocsStore } from "../hooks/useWikiDocs";
import { mergeLibrary } from "../../../shared-ts/library";
import { repoRootPath } from "../../../shared-ts/repoPath";
import type { Artifact } from "./ArtifactsView";
import { buildHomeRows, rowSessionId, type HomeRow } from "./homeRows";
import { isMachineWideStore, type WorkspaceMemory } from "./MemoryView";
import type { ProjectTab } from "./ProjectsView";
import { STATE_DOT, formatSpan } from "./TaskList";
import { timeAgo } from "./timeAgo";
import {
  OVERVIEW_RECENT_LIMIT,
  overviewLibrary,
  overviewRunningProcs,
  overviewTasks,
} from "./projectOverviewData";
import styles from "./ProjectOverview.module.css";

/**
 * A project's first tab: what is going on in it right now. Every block is a
 * slice of data another page already owns (tasks, commands, library, memory,
 * routines) and links over to that page; a block with nothing in it shrinks
 * to its link instead of leaving an empty panel.
 */
export function ProjectOverview({
  projectPath,
  onTab,
}: {
  projectPath: string;
  onTab: (tab: ProjectTab) => void;
}) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const decisions = useDecisionStore((s) => s.decisions);
  const procs = useProcStore((s) => s.procs);
  const ui = useUIStore.getState;

  const rows = useMemo(
    () =>
      buildHomeRows(sessions, decisions, {
        isQuiet: (s) => isQuietAliveSticky(s),
        isWaking: (s, pending) => isWakingSticky(s, pending),
        untitled: t("history.untitled"),
      }),
    [sessions, decisions, t],
  );
  const tasks = useMemo(() => overviewTasks(rows, projectPath), [rows, projectPath]);
  const running = useMemo(() => overviewRunningProcs(procs, projectPath), [procs, projectPath]);

  const docs = useWikiDocsStore((s) => s.docs);
  const docsLoaded = useWikiDocsStore((s) => s.loaded);
  const fetchDocs = useWikiDocsStore((s) => s.fetch);
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const [memoryCount, setMemoryCount] = useState<number | null>(null);
  const [routineCount, setRoutineCount] = useState<number | null>(null);
  useEffect(() => {
    let alive = true;
    if (!docsLoaded) void fetchDocs();
    invoke<Artifact[]>("list_artifacts")
      .then((rows) => alive && setArtifacts(rows ?? []))
      .catch(() => alive && setArtifacts([]));
    // Same project match as the memory tab (`ProjectMemory`).
    invoke<WorkspaceMemory[]>("list_memories")
      .then((ws) => {
        if (!alive) return;
        const mine = (ws ?? []).filter(
          (w) => !isMachineWideStore(w) && repoRootPath(w.workspacePath) === projectPath,
        );
        setMemoryCount(mine.reduce((n, w) => n + w.files.length, 0));
      })
      .catch(() => alive && setMemoryCount(0));
    // Same project match as the routines tab (`RoutinesPanel`); a fired
    // one-shot schedule is history, not a routine.
    Promise.all([
      invoke<Array<{ workspacePath: string }>>("list_loops"),
      invoke<Array<{ workspacePath: string; status: string }>>("list_schedules"),
    ])
      .then(([loops, schedules]) => {
        if (!alive) return;
        const mine = (w: string) => repoRootPath(w) === projectPath;
        setRoutineCount(
          (loops ?? []).filter((l) => mine(l.workspacePath)).length +
            (schedules ?? []).filter((s) => s.status === "pending" && mine(s.workspacePath)).length,
        );
      })
      .catch(() => alive && setRoutineCount(0));
    return () => {
      alive = false;
    };
    // fetchDocs / docsLoaded: the shared wiki list loads once app-wide.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectPath]);

  const library = useMemo(
    () => (docsLoaded && artifacts ? overviewLibrary(mergeLibrary(docs, artifacts), projectPath) : null),
    [docs, docsLoaded, artifacts, projectPath],
  );

  const now = Date.now();
  const openTask = (row: HomeRow) => ui().requestOpenTask(rowSessionId(row));
  const allTasks = () => {
    ui().setHistoryWorkspaceFilter(projectPath);
    ui().setViewMode("history");
  };
  const newTask = () => ui().requestNewSession({ prompt: "", workspace: projectPath });
  const openLibrary = (selectedKey: string | null) => {
    ui().updateMainViewState("library", { project: projectPath, query: "", selectedKey });
    ui().setViewMode("library");
  };

  const taskRow = (row: HomeRow) => (
    <button key={row.key} type="button" className={styles.task} onClick={() => openTask(row)}>
      <span className={styles.dot} style={{ background: STATE_DOT[row.state] }} />
      <span className={styles.task_title}>{row.title}</span>
      <span className={styles.task_state}>{t(`home.section_${row.state}`)}</span>
      <span className={styles.task_time}>
        {row.state === "needsYou" || row.state === "running"
          ? formatSpan(now - row.startedAtMs, t)
          : timeAgo(row.lastActivityMs, t)}
      </span>
    </button>
  );

  return (
    <div className={styles.root} data-testid="project-overview">
      <Section
        title={t("projects.ov_open", "进行中的任务")}
        count={tasks.open.length}
        actions={
          <>
            <button type="button" className={styles.link} onClick={newTask}>
              <Plus size={12} strokeWidth={2} />
              {t("projects.ov_new_task", "新任务")}
            </button>
            <button type="button" className={styles.link} onClick={allTasks}>
              {t("projects.ov_all_tasks", "在任务页查看")}
              <ChevronRight size={12} strokeWidth={2} />
            </button>
          </>
        }
      >
        {tasks.open.length > 0 ? (
          tasks.open.map(taskRow)
        ) : (
          <div className={styles.none}>{t("projects.ov_no_open", "眼下没有在跑的任务")}</div>
        )}
      </Section>

      {tasks.recentDone.length > 0 && (
        <Section
          title={t("projects.ov_recent_done", "最近完成")}
          count={tasks.doneTotal}
          actions={
            tasks.doneTotal > OVERVIEW_RECENT_LIMIT && (
              <button type="button" className={styles.link} onClick={allTasks}>
                {t("projects.ov_all_done", "全部 {{count}} 个", { count: tasks.doneTotal })}
                <ChevronRight size={12} strokeWidth={2} />
              </button>
            )
          }
        >
          {tasks.recentDone.map(taskRow)}
        </Section>
      )}

      {(running.length > 0 || (library && library.length > 0)) && (
        <div className={styles.pair}>
          {running.length > 0 && (
            <Section
              title={t("projects.ov_running_cmds", "正在跑的命令")}
              count={running.length}
              actions={
                <button type="button" className={styles.link} onClick={() => onTab("commands")}>
                  {t("projects.tab_commands", "命令")}
                  <ChevronRight size={12} strokeWidth={2} />
                </button>
              }
            >
              {running.slice(0, OVERVIEW_RECENT_LIMIT).map((p) => (
                <button key={p.id} type="button" className={styles.item} onClick={() => onTab("commands")}>
                  <SquareTerminal size={13} strokeWidth={1.7} className={styles.item_icon} />
                  <span className={styles.item_mono}>{p.command}</span>
                  <span className={styles.task_time}>{formatSpan(now - p.startedMs, t)}</span>
                </button>
              ))}
            </Section>
          )}
          {library && library.length > 0 && (
            <Section
              title={t("projects.ov_library", "资料")}
              count={library.length}
              actions={
                <button type="button" className={styles.link} onClick={() => openLibrary(null)}>
                  {t("projects.ov_all_library", "在资料页查看")}
                  <ChevronRight size={12} strokeWidth={2} />
                </button>
              }
            >
              {library.slice(0, OVERVIEW_RECENT_LIMIT).map((it) => (
                <button key={it.key} type="button" className={styles.item} onClick={() => openLibrary(it.key)}>
                  {it.type === "doc" ? (
                    <BookOpen size={13} strokeWidth={1.7} className={styles.item_icon} />
                  ) : (
                    <FileText size={13} strokeWidth={1.7} className={styles.item_icon} />
                  )}
                  <span className={styles.item_title} title={it.title}>
                    {it.title}
                  </span>
                  <span className={styles.task_time}>{timeAgo(it.updatedMs, t)}</span>
                </button>
              ))}
            </Section>
          )}
        </div>
      )}

      <div className={styles.summary}>
        <button type="button" className={styles.chip} onClick={() => onTab("memory")}>
          <Brain size={13} strokeWidth={1.7} />
          {t("projects.tab_memory", "记忆")}
          <span className={styles.chip_count}>{memoryCount ?? "…"}</span>
        </button>
        <button type="button" className={styles.chip} onClick={() => onTab("routines")}>
          <CalendarClock size={13} strokeWidth={1.7} />
          {routineCount === 0 ? (
            t("projects.ov_routines_none", "还没有例行")
          ) : (
            <>
              {t("projects.tab_routines", "例行")}
              <span className={styles.chip_count}>{routineCount ?? "…"}</span>
            </>
          )}
        </button>
      </div>
    </div>
  );
}

function Section({
  title,
  count,
  actions,
  children,
}: {
  title: string;
  count: number;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className={styles.section}>
      <div className={styles.section_head}>
        <span className={styles.section_title}>{title}</span>
        <span className={styles.section_count}>{count}</span>
        <span className={styles.section_actions}>{actions}</span>
      </div>
      <div className={styles.section_body}>{children}</div>
    </section>
  );
}
