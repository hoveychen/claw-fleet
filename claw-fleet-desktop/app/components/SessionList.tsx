import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Menu, Shield, Plus, ListTodo, FolderGit2, Rocket } from "lucide-react";
import { openSettingsWindow, useAuditStore, useConnectionStore, useDetailStore, useFleetManagedStore, useProjectsStore, useSessionsStore, useTasksStore, useUIStore } from "../store";
import { isWorkflowAgent } from "../workflowAgent";
import type { SessionInfo, TaskStatus } from "../types";
import { GalleryView } from "./GalleryView";
import { SessionEmptyState } from "./EmptyState";
import { MascotEyes } from "./MascotEyes";
import { useUsageRing } from "../hooks/useUsageRing";
import { MemoryView } from "./MemoryView";
import { AuditView } from "./AuditView";
import { ReportView } from "./report/ReportView";
import { SkillsView } from "./SkillsView";
import { PluginsView } from "./PluginsView";
import { ProjectsView } from "./ProjectsView";
import { TasksView } from "./TasksView";
import { SessionLauncher, type SessionLauncherProps } from "./SessionLauncher";
import { SessionCard } from "./SessionCard";
import { SessionToolbar } from "./SessionToolbar";
import { MobileAccessPanel } from "./MobileAccessPanel";
import { HistoryView } from "./HistoryView";
import styles from "./SessionList.module.css";
import { LiveStats } from "./LiveStats";
import { UsagePanel } from "./UsagePanel";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { getItem, setItem } from "../storage";
import { PROJECTS_FEATURE_ENABLED } from "../featureFlags";

const MIN_WIDTH = 200;
const MAX_WIDTH = 520;
const DEFAULT_WIDTH = 280;

/** Status-dot colour for a task row in the sidebar's Tasks section. Mirrors
 *  the StatusBadge palette used on the detail page. */
function taskStatusColor(status: TaskStatus): string {
  switch (status) {
    case "running":
      return "#f0875a";
    case "reviewing":
    case "awaitingAcceptance":
      return "#7aa0fa";
    case "paused":
      return "#d0a85a";
    case "drafting":
      return "#9a9aa0";
    case "done":
      return "#5ac88c";
    default:
      return "#6b6b72"; // abandoned
  }
}

function getSavedWidth(): number {
  const saved = getItem("sidebar-width");
  if (saved) {
    const n = parseInt(saved, 10);
    if (n >= MIN_WIDTH && n <= MAX_WIDTH) return n;
  }
  return DEFAULT_WIDTH;
}

export function SessionList() {
  const { t } = useTranslation();
  const { sessions, refresh, setSessions, scanReady, setScanReady } = useSessionsStore();
  const { session: viewedSession, open } = useDetailStore();
  const {
    viewMode,
    setViewMode,
    lastSessionViewMode,
    setLiteMode,
    sidebarCollapsed,
    setSidebarCollapsed,
    mascotVisible,
    showMobileAccess,
    setShowMobileAccess,
    setShowNewProjectRequested,
  } = useUIStore();
  const isSessionView = viewMode === "list" || viewMode === "gallery";
  // Top-level sidebar tab derived from viewMode (no separate persisted state):
  // the projects/tasks views live under the "tasks" tab, everything else
  // (sessions / audit / report / memory / skills / plugins) under "monitor".
  const sidebarTab: "monitor" | "tasks" =
    PROJECTS_FEATURE_ENABLED && (viewMode === "tasks" || viewMode === "projects")
      ? "tasks"
      : "monitor";
  const { connection } = useConnectionStore();
  const unreadCriticalCount = useAuditStore((s) => s.unreadCriticalCount);
  const [filter, setFilter] = useState("");
  const [showAll, setShowAll] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(getSavedWidth);
  const [launcherInitial, setLauncherInitial] = useState<SessionLauncherProps["initial"]>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [mobileActive, setMobileActive] = useState(false);
  const projects = useProjectsStore((s) => s.projects);
  const tasks = useTasksStore((s) => s.tasks);
  const selectedProjectId = useProjectsStore((s) => s.selectedProjectId);
  const selectedTaskId = useTasksStore((s) => s.selectedTaskId);
  const setSelectedProjectId = useProjectsStore((s) => s.setSelectedProjectId);
  const setSelectedTaskId = useTasksStore((s) => s.setSelectedTaskId);
  const usageRing = useUsageRing();

  // Tasks section in the sidebar lists every task (across all projects),
  // sorted so the things needing attention float to the top: running /
  // reviewing / awaiting first, then drafting / paused, then terminal
  // (done / abandoned). Ties broken by most-recent activity.
  const taskSortRank = (s: TaskStatus): number => {
    switch (s) {
      case "running":
      case "reviewing":
      case "awaitingAcceptance":
        return 0;
      case "paused":
      case "drafting":
        return 1;
      default:
        return 2; // done / abandoned
    }
  };
  const allTasksSorted = [...tasks].sort((a, b) => {
    const r = taskSortRank(a.status) - taskSortRank(b.status);
    if (r !== 0) return r;
    return (b.startedAt ?? b.createdAt) - (a.startedAt ?? a.createdAt);
  });

  const openTask = useCallback(
    (taskId: string, projectId: string) => {
      setSelectedProjectId(projectId);
      setSelectedTaskId(taskId);
      setViewMode("tasks");
    },
    [setSelectedProjectId, setSelectedTaskId, setViewMode],
  );

  // Click a project in the PROJECTS section → open that project's page
  // (task list + search/filter). Clears selectedTaskId so a stale task
  // detail doesn't linger.
  const openProject = useCallback(
    (projectId: string) => {
      setSelectedProjectId(projectId);
      setSelectedTaskId(null);
      setViewMode("projects");
    },
    [setSelectedProjectId, setSelectedTaskId, setViewMode],
  );

  // Click "+ New task" → land on the Tasks page with no task selected, which
  // renders the centered composer (the default new-task entry point).
  const openNewTask = useCallback(() => {
    setSelectedTaskId(null);
    setViewMode("tasks");
  }, [setSelectedTaskId, setViewMode]);

  // Sidebar 5s coalesced poller: mobile-access status indicator + projects
  // store + tasks store. Three independent setInterval timers used to fire on
  // their own 5s cadence (mobile / projects / tasks); merged into a single
  // tick so the WebContent main thread only does one wave of state updates per
  // 5s instead of three closely-spaced ones. Skips while the page is hidden.
  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      if (cancelled || document.visibilityState === "hidden") return;
      invoke<{ running: boolean; tunnelUrl: string | null }>("get_mobile_access_status")
        .then((s) => { if (!cancelled) setMobileActive(s.running && !!s.tunnelUrl); })
        .catch(() => {});
      if (PROJECTS_FEATURE_ENABLED) {
        useProjectsStore.getState().refresh();
        useTasksStore.getState().refresh();
      }
    };
    // Kick off once on mount (mobile-status, projects, tasks all want a
    // first read before the 5s wait).
    tick();
    const interval = setInterval(tick, 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);
  // Projects feature is hidden — bounce returning users off a persisted
  // `projects` / `tasks` viewMode so they don't land on an inaccessible view.
  useEffect(() => {
    if (!PROJECTS_FEATURE_ENABLED && (viewMode === "projects" || viewMode === "tasks")) {
      setViewMode("gallery");
    }
  }, [viewMode, setViewMode]);

  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    useFleetManagedStore.getState().refresh();
    // Load audit data for the unread critical badge
    invoke<import("../types").AuditSummary>("get_audit_events")
      .then((data) => {
        useAuditStore.getState().setCriticalEvents(
          data.events.filter((e) => e.riskLevel === "critical")
        );
      })
      .catch(() => {});
    // Register event listeners BEFORE calling refresh() to avoid a race
    // condition: on Linux the initial background scan can complete so fast
    // that the "sessions-updated" event fires before the listener is set up,
    // causing existing sessions to be invisible until a new one is created.
    const unlistenPromise = listen<SessionInfo[]>("sessions-updated", (e) => {
      setSessions(e.payload);
    });
    const unlistenScanReady = listen<boolean>("scan-ready", () => {
      setScanReady(true);
    });
    // Refresh after listeners are registered. Even if the initial scan event
    // was already emitted, this fetch will pick up whatever has been scanned
    // so far; and any future events will be caught by the listeners above.
    unlistenPromise.then(() => refresh());
    unlistenScanReady.then(() => refresh());
    return () => {
      unlistenPromise.then((u) => u());
      unlistenScanReady.then((u) => u());
    };
  }, []);

  const handleResizeMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    dragRef.current = { startX: e.clientX, startWidth: sidebarWidth };
    setIsDragging(true);

    function onMouseMove(e: MouseEvent) {
      if (!dragRef.current) return;
      const delta = e.clientX - dragRef.current.startX;
      const newWidth = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, dragRef.current.startWidth + delta));
      setSidebarWidth(newWidth);
    }

    function onMouseUp() {
      dragRef.current = null;
      setIsDragging(false);
      setSidebarWidth((prev) => {
        setItem("sidebar-width", String(prev));
        return prev;
      });
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
    }

    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  }, [sidebarWidth]);

  const { searching, ftsMatchPaths } = useSessionSearch(filter);

  const filtered = sessions.filter((s) => {
    if (isWorkflowAgent(s)) return false; // hidden from the list (open via DAG node only)
    if (!filter) return true;
    const q = filter.toLowerCase();
    const clientMatch =
      s.workspaceName.toLowerCase().includes(q) ||
      s.slug?.toLowerCase().includes(q) ||
      s.agentDescription?.toLowerCase().includes(q) ||
      s.ideName?.toLowerCase().includes(q);
    return clientMatch || ftsMatchPaths.has(s.jsonlPath);
  });

  // Promote idle main sessions that have active subagents → delegating
  const activeSubagentParentIds = new Set(
    filtered
      .filter(
        (s) =>
          s.isSubagent &&
          s.parentSessionId &&
          ["thinking", "executing", "streaming", "processing", "waitingInput", "active"].includes(s.status)
      )
      .map((s) => s.parentSessionId!)
  );
  const promoted = filtered.map((s) =>
    !s.isSubagent &&
    ["idle", "active", "waitingInput", "processing"].includes(s.status) &&
    activeSubagentParentIds.has(s.id)
      ? { ...s, status: "delegating" as const }
      : s
  );

  const active = promoted.filter((s) =>
    ["thinking", "executing", "streaming", "processing", "waitingInput", "active", "delegating"].includes(s.status)
  );
  const idle = promoted
    .filter((s) => s.status === "idle")
    .sort((a, b) => b.lastActivityMs - a.lastActivityMs);

  function buildTree(list: SessionInfo[]) {
    const mains = list.filter((s) => !s.isSubagent);
    const subagentsByParent = new Map<string, SessionInfo[]>();
    for (const s of list) {
      if (s.isSubagent && s.parentSessionId) {
        const arr = subagentsByParent.get(s.parentSessionId) ?? [];
        arr.push(s);
        subagentsByParent.set(s.parentSessionId, arr);
      }
    }
    const orphans = list.filter(
      (s) =>
        s.isSubagent &&
        (!s.parentSessionId ||
          !subagentsByParent.has(s.parentSessionId) ||
          !mains.find((m) => m.id === s.parentSessionId))
    );
    const result: { session: SessionInfo; indented: boolean; extend: boolean }[] = [];
    for (const main of mains) {
      result.push({ session: main, indented: false, extend: false });
      for (const sub of subagentsByParent.get(main.id) ?? []) {
        result.push({ session: sub, indented: true, extend: false });
      }
    }
    for (const orphan of orphans) {
      result.push({ session: orphan, indented: false, extend: false });
    }
    // Mark each indented row whose successor is also indented so the L-bracket
    // thread runs through the full height instead of stopping at the elbow.
    for (let i = 0; i < result.length - 1; i++) {
      if (result[i].indented && result[i + 1].indented) {
        result[i].extend = true;
      }
    }
    return result;
  }

  function renderGroup(list: SessionInfo[]) {
    return buildTree(list).map(({ session: s, indented, extend }) => {
      const className = indented
        ? `${styles.indented}${extend ? ` ${styles.indented_extend}` : ""}`
        : undefined;
      return (
        <div key={s.jsonlPath} className={className}>
          <SessionCard
            session={s}
            isSelected={viewedSession?.jsonlPath === s.jsonlPath}
            onClick={() => {
              const isFtsHit = filter.trim().length >= 2 && ftsMatchPaths.has(s.jsonlPath);
              open(s, isFtsHit ? filter.trim() : undefined);
            }}
          />
        </div>
      );
    });
  }

  const isRemote = connection?.type === "remote";

  const COLLAPSED_WIDTH = 64;
  const effectiveWidth = sidebarCollapsed ? COLLAPSED_WIDTH : sidebarWidth;

  return (
    <>
      <aside
        className={`${styles.sidebar}${sidebarCollapsed ? ` ${styles.sidebar_collapsed}` : ""}`}
        style={{ width: effectiveWidth }}
      >
        {/* Empty header strip — reserves the top-right space for the collapse
            toggle and provides a drag region around the macOS traffic lights. */}
        <div className={styles.header} data-tauri-drag-region />

        {/* Collapse / expand toggle — absolute top-right of the sidebar. */}
        <button
          type="button"
          className={styles.sidebar_toggle}
          onClick={() => setSidebarCollapsed(!sidebarCollapsed)}
          title={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
          aria-label={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
            <rect x="1.5" y="2.5" width="13" height="11" rx="1.3" />
            <rect x="10" y="2.5" width="4.5" height="11" rx="1.3" fill="currentColor" fillOpacity="0.35" stroke="none" />
            <line x1="10" y1="2.5" x2="10" y2="13.5" />
          </svg>
        </button>

        {/* Unified sidebar nav — top-level Monitor/Tasks tabs, then the
            active tab's contents. */}
        <nav className={`${styles.nav}${sidebarCollapsed ? ` ${styles.nav_collapsed}` : ""}`} data-wizard="view-toggle">
          {/* Top-level tabs: Monitor group vs Tasks group. Shown only when the
              projects/tasks feature is enabled; otherwise the nav is just the
              monitor items. */}
          {PROJECTS_FEATURE_ENABLED && (
            <div className={`${styles.sidebar_tabs}${sidebarCollapsed ? ` ${styles.sidebar_tabs_collapsed}` : ""}`}>
              <button
                type="button"
                className={`${styles.sidebar_tab} ${sidebarTab === "monitor" ? styles.sidebar_tab_active : ""}`}
                onClick={() => {
                  if (sidebarTab !== "monitor") setViewMode(lastSessionViewMode);
                }}
                title={t("sidebar.tab_monitor", "会话")}
              >
                {sidebarCollapsed
                  ? <Menu size={15} strokeWidth={1.6} />
                  : <span className={styles.sidebar_tab_label}>{t("sidebar.tab_monitor", "会话")}</span>}
              </button>
              <button
                type="button"
                className={`${styles.sidebar_tab} ${sidebarTab === "tasks" ? styles.sidebar_tab_active : ""}`}
                onClick={() => {
                  if (sidebarTab !== "tasks") setViewMode("projects");
                }}
                title={t("sidebar.tab_tasks", "任务")}
              >
                {sidebarCollapsed
                  ? <ListTodo size={15} strokeWidth={1.6} />
                  : <span className={styles.sidebar_tab_label}>{t("sidebar.tab_tasks", "任务")}</span>}
              </button>
            </div>
          )}

          {/* ── Monitor tab: sessions / audit / report + memory / skills / plugins ── */}
          {sidebarTab === "monitor" && (
            <>
              <button
                className={`${styles.nav_item} ${isSessionView ? styles.nav_active : ""}`}
                onClick={() => {
                  if (!isSessionView) setViewMode(lastSessionViewMode);
                }}
              >
                <span className={styles.nav_icon}><Menu size={14} strokeWidth={1.5} /></span>
                <span className={styles.nav_label}>{t("view_sessions")}</span>
              </button>
              <button
                className={`${styles.nav_item} ${viewMode === "history" ? styles.nav_active : ""}`}
                onClick={() => setViewMode("history")}
              >
                <span className={styles.nav_icon}><Rocket size={14} strokeWidth={1.5} /></span>
                <span className={styles.nav_label}>{t("view_history", "启动台")}</span>
              </button>
              <button
                className={`${styles.nav_item} ${viewMode === "audit" ? styles.nav_active : ""}`}
                onClick={() => setViewMode("audit")}
              >
                <span className={styles.nav_icon}><Shield size={14} strokeWidth={1.5} /></span>
                <span className={styles.nav_label}>{t("view_audit")}</span>
                {unreadCriticalCount > 0 && (
                  <span className={styles.nav_badge}>{unreadCriticalCount}</span>
                )}
              </button>
              <button
                className={`${styles.nav_item} ${viewMode === "report" ? styles.nav_active : ""}`}
                onClick={() => setViewMode("report")}
              >
                <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3"><rect x="1.5" y="2.5" width="13" height="12" rx="1.5"/><line x1="1.5" y1="5.5" x2="14.5" y2="5.5"/><line x1="5" y1="1" x2="5" y2="4"/><line x1="11" y1="1" x2="11" y2="4"/></svg></span>
                <span className={styles.nav_label}>{t("view_report")}</span>
              </button>

              <div className={styles.nav_divider} />

              <button
                className={`${styles.nav_item} ${viewMode === "memory" ? styles.nav_active : ""}`}
                onClick={() => setViewMode("memory")}
              >
                <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M4 2.5h6.5a2 2 0 0 1 2 2v9a1 1 0 0 1-1 1H4a1.5 1.5 0 0 1-1.5-1.5V4A1.5 1.5 0 0 1 4 2.5Z"/><path d="M2.5 11.5H12"/><path d="M5.5 5.5h4"/><path d="M5.5 7.5h3"/></svg></span>
                <span className={styles.nav_label}>{t("view_memory")}</span>
              </button>
              <button
                className={`${styles.nav_item} ${viewMode === "skills" ? styles.nav_active : ""}`}
                onClick={() => setViewMode("skills")}
              >
                <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M9 1.5 3.5 9h4L7 14.5 12.5 7h-4L9 1.5Z"/></svg></span>
                <span className={styles.nav_label}>{t("view_skills")}</span>
              </button>
              <button
                className={`${styles.nav_item} ${viewMode === "plugins" ? styles.nav_active : ""}`}
                onClick={() => setViewMode("plugins")}
              >
                <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M5.5 1.5v3h-3v3.5a2 2 0 0 0 2 2H6v3a2 2 0 0 0 2 2 2 2 0 0 0 2-2v-3h1.5a2 2 0 0 0 2-2v-3.5h-3v-3a2 2 0 0 0-2-2 2 2 0 0 0-2 2Z"/></svg></span>
                <span className={styles.nav_label}>{t("view_plugins")}</span>
              </button>
            </>
          )}

          {/* ── Tasks tab: two always-expanded sections — Projects + Tasks ── */}
          {PROJECTS_FEATURE_ENABLED && sidebarTab === "tasks" && !sidebarCollapsed && (
            <>
              {/* PROJECTS section — every project as a clickable row. Click
                  opens the project's page (task list + search). */}
              <div className={styles.nav_section_header}>
                <span className={styles.nav_section_title}>{t("sidebar.section_projects", "Projects")}</span>
                <span className={styles.nav_section_count}>{projects.length}</span>
              </div>
              {projects.length === 0 && (
                <div className={styles.nav_empty}>{t("sidebar.no_projects", "No projects yet")}</div>
              )}
              {projects.map((p) => {
                const isProjectActive = viewMode === "projects" && selectedProjectId === p.id;
                const projectTaskCount = tasks.filter((tk) => tk.projectId === p.id).length;
                return (
                  <button
                    key={p.id}
                    type="button"
                    className={`${styles.nav_project_item} ${isProjectActive ? styles.nav_project_item_active : ""}`}
                    onClick={() => openProject(p.id)}
                    title={p.workspace}
                  >
                    <span className={styles.nav_task_icon}><FolderGit2 size={12} strokeWidth={1.6} /></span>
                    <span className={styles.nav_project_name}>{p.name}</span>
                    {projectTaskCount > 0 && (
                      <span className={styles.nav_project_chip}>{projectTaskCount}</span>
                    )}
                  </button>
                );
              })}
              <button
                type="button"
                className={styles.nav_section_add}
                onClick={() => {
                  setShowNewProjectRequested(true);
                  setViewMode("projects");
                }}
                title={t("projects.new")}
              >
                <Plus size={13} strokeWidth={1.6} />
                <span>{t("projects.new")}</span>
              </button>

              <div className={styles.nav_divider} />

              {/* TASKS section — every task across all projects. Click opens
                  the task detail. */}
              <div className={styles.nav_section_header}>
                <span className={styles.nav_section_title}>{t("sidebar.section_tasks", "Tasks")}</span>
                <span className={styles.nav_section_count}>{tasks.length}</span>
              </div>
              {tasks.length === 0 && (
                <div className={styles.nav_empty}>{t("sidebar.no_tasks", "No tasks yet")}</div>
              )}
              {allTasksSorted.map((tk) => {
                const isTaskActive = viewMode === "tasks" && selectedTaskId === tk.id;
                const proj = projects.find((p) => p.id === tk.projectId);
                const items = Object.values(tk.plan.items ?? {});
                const total = items.length;
                const done = items.filter(
                  (it) => typeof it.status === "string" && (it.status === "done" || it.status === "skipped"),
                ).length;
                return (
                  <button
                    key={tk.id}
                    type="button"
                    className={`${styles.nav_project_item} ${isTaskActive ? styles.nav_project_item_active : ""}`}
                    onClick={() => openTask(tk.id, tk.projectId)}
                    title={`${tk.title} — ${tk.status}`}
                  >
                    <span
                      className={styles.nav_task_status_dot}
                      style={{ background: taskStatusColor(tk.status) }}
                    />
                    <span className={styles.nav_project_name}>{tk.title}</span>
                    {total > 0 && (
                      <span className={styles.nav_project_chip} title={t("tasks.progress", "p-items done / total")}>
                        {done}/{total}
                      </span>
                    )}
                    {proj && <span className={styles.nav_task_project_chip}>{proj.name}</span>}
                  </button>
                );
              })}
              <button
                type="button"
                className={styles.nav_section_add}
                onClick={openNewTask}
                disabled={projects.length === 0}
                title={projects.length === 0 ? t("tasks.create_project_first", "Create a project first") : t("tasks.new_task", "New task")}
              >
                <Plus size={13} strokeWidth={1.6} />
                <span>{t("tasks.new_task", "New task")}</span>
              </button>
            </>
          )}
        </nav>

        <div className={styles.separator} />

        {/* Scrollable sidebar content — charts + usage hidden for the
            task-focused views (projects / tasks) to keep that rail clean. */}
        <div className={styles.sidebar_content}>
          {viewMode !== "projects" && viewMode !== "tasks" && (
            <>
              <LiveStats collapsed={sidebarCollapsed} />
              <UsagePanel collapsed={sidebarCollapsed} />
            </>
          )}

          {!sidebarCollapsed && mascotVisible && (
            <div className={styles.mascot_section}>
              <MascotEyes
                dashboardMode
                usageRing={usageRing ? {
                  percent: usageRing.overall,
                  topSource: usageRing.topSource,
                  sources: usageRing.sources,
                } : null}
              />
            </div>
          )}
        </div>

        {/* Footer profile card */}
        <div className={styles.footer} data-wizard="settings-footer">
          <button
            className={styles.footer_card}
            onClick={openSettingsWindow}
            title={t("settings.title")}
          >
            <div className={styles.footer_avatar}>
              <img src="/app-icon.png" alt="" style={{ width: "100%", height: "100%", objectFit: "contain" }} />
            </div>
            {!sidebarCollapsed && (
              <>
                <div className={styles.footer_info}>
                  <span className={styles.footer_name}>{t("title")}</span>
                  <span className={styles.footer_status}>
                    <span className={`${styles.footer_dot} ${isRemote ? styles.footer_dot_remote : ""}`} />
                    {isRemote ? t("settings.remote") : t("settings.local")}
                  </span>
                </div>
                <button
                  className={styles.footer_mobile_btn}
                  onClick={(e) => { e.stopPropagation(); setLiteMode(true); }}
                  title={t("lite.enter")}
                >
                  {/* Picture-in-picture / mini-window glyph for Lite mode */}
                  <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
                    <rect x="1.5" y="2.5" width="13" height="11" rx="1.3" />
                    <rect x="8.5" y="7.5" width="5" height="4.5" rx="0.8" fill="currentColor" fillOpacity="0.4" />
                  </svg>
                </button>
                <button
                  className={`${styles.footer_mobile_btn} ${mobileActive ? styles.footer_mobile_active : ""}`}
                  onClick={(e) => { e.stopPropagation(); setShowMobileAccess(true); }}
                  title={t("settings.mobile_access")}
                >
                  <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
                    <rect x="4" y="1" width="8" height="14" rx="1.5" />
                    <line x1="7" y1="12" x2="9" y2="12" />
                  </svg>
                  {mobileActive && <span className={styles.footer_mobile_dot} />}
                </button>
              </>
            )}
            <span className={styles.footer_gear}>⚙</span>
          </button>
        </div>

        {/* Resize handle — hidden when collapsed */}
        {!sidebarCollapsed && (
          <div
            className={`${styles.resize_handle} ${isDragging ? styles.resize_handle_active : ""}`}
            onMouseDown={handleResizeMouseDown}
          />
        )}
      </aside>

      {/* Mobile access panel */}
      {showMobileAccess && <MobileAccessPanel onClose={() => setShowMobileAccess(false)} />}

      {/* Main content area */}
      {viewMode === "list" ? (
        <div className={styles.main_area}>
          <SessionToolbar
            filter={filter}
            onFilterChange={setFilter}
            activeCount={active.length}
            totalCount={sessions.length}
            showAll={showAll}
            onToggleShowAll={() => setShowAll((v) => !v)}
            ftsMatchCount={filter.trim().length >= 2 ? ftsMatchPaths.size : undefined}
            searching={searching}
          />

          <div className={styles.list}>
            {showAll ? (
              <>
                {active.length > 0 && (
                  <section>
                    <div className={styles.group_label}>{t("active")}</div>
                    {renderGroup(active)}
                  </section>
                )}
                {idle.length > 0 && (
                  <section>
                    <div className={styles.group_label}>{t("recent")}</div>
                    {renderGroup(idle)}
                  </section>
                )}
                {promoted.length === 0 && (
                  <SessionEmptyState scanReady={scanReady} hasSessions={sessions.length > 0} />
                )}
              </>
            ) : (
              <>
                {active.length > 0 && renderGroup(active)}
                {active.length === 0 && (
                  <SessionEmptyState scanReady={scanReady} hasSessions={sessions.length > 0} />
                )}
              </>
            )}
          </div>
        </div>
      ) : viewMode === "gallery" ? (
        <GalleryView />
      ) : viewMode === "history" ? (
        <HistoryView />
      ) : viewMode === "audit" ? (
        <AuditView />
      ) : viewMode === "memory" ? (
        <MemoryView />
      ) : viewMode === "skills" ? (
        <SkillsView />
      ) : viewMode === "plugins" ? (
        <PluginsView />
      ) : viewMode === "projects" && PROJECTS_FEATURE_ENABLED ? (
        <ProjectsView />
      ) : viewMode === "tasks" && PROJECTS_FEATURE_ENABLED ? (
        <TasksView />
      ) : (
        <ReportView />
      )}

      <SessionLauncher
        initial={launcherInitial}
        onClose={() => setLauncherInitial(null)}
      />
    </>
  );
}
