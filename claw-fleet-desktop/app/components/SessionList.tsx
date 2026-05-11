import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { openSettingsWindow, useAuditStore, useConnectionStore, useDetailStore, useFleetManagedStore, useProjectsStore, useSessionsStore, useUIStore } from "../store";
import type { SessionInfo } from "../types";
import { GalleryView } from "./GalleryView";
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
import styles from "./SessionList.module.css";
import { TokenSpeedChart } from "./TokenSpeedChart";
import { CostSpeedChart } from "./CostSpeedChart";
import { UsagePanel } from "./UsagePanel";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { getItem, setItem } from "../storage";

const MIN_WIDTH = 200;
const MAX_WIDTH = 520;
const DEFAULT_WIDTH = 280;

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
  const { viewMode, setViewMode, setLiteMode, sidebarCollapsed, setSidebarCollapsed, showMobileAccess, setShowMobileAccess } = useUIStore();
  const { connection } = useConnectionStore();
  const unreadCriticalCount = useAuditStore((s) => s.unreadCriticalCount);
  const [filter, setFilter] = useState("");
  const [showAll, setShowAll] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(getSavedWidth);
  const [launcherInitial, setLauncherInitial] = useState<SessionLauncherProps["initial"]>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [isWindows, setIsWindows] = useState(false);
  const [mobileActive, setMobileActive] = useState(false);
  const [projectsExpanded, setProjectsExpanded] = useState<boolean>(
    () => getItem("nav-projects-expanded") === "true",
  );
  const projects = useProjectsStore((s) => s.projects);
  const fleetSessions = useProjectsStore((s) => s.fleetSessions);
  const setSelectedProjectId = useProjectsStore((s) => s.setSelectedProjectId);
  const usageRing = useUsageRing();

  const projectCounts = (() => {
    const total = new Map<string, number>();
    const running = new Map<string, number>();
    for (const fs of fleetSessions) {
      total.set(fs.projectId, (total.get(fs.projectId) ?? 0) + 1);
      if (fs.status === "running") {
        running.set(fs.projectId, (running.get(fs.projectId) ?? 0) + 1);
      }
    }
    return { total, running };
  })();

  const toggleProjectsExpanded = useCallback(() => {
    setProjectsExpanded((prev) => {
      const next = !prev;
      setItem("nav-projects-expanded", next ? "true" : "false");
      return next;
    });
  }, []);

  const openProject = useCallback(
    (projectId: string) => {
      setSelectedProjectId(projectId);
      setViewMode("projects");
    },
    [setSelectedProjectId, setViewMode],
  );

  // Poll mobile access status for the sidebar indicator.
  useEffect(() => {
    const check = () => {
      invoke<{ running: boolean; tunnelUrl: string | null }>("get_mobile_access_status")
        .then((s) => setMobileActive(s.running && !!s.tunnelUrl))
        .catch(() => {});
    };
    check();
    const interval = setInterval(check, 5000);
    return () => clearInterval(interval);
  }, []);

  // Keep the projects store fresh so the sidebar nav can show per-project
  // session counts whether or not the Projects view is mounted.
  useEffect(() => {
    const projectsStore = useProjectsStore.getState();
    projectsStore.refresh();
    const interval = setInterval(() => {
      useProjectsStore.getState().refresh();
    }, 5000);
    return () => clearInterval(interval);
  }, []);
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    invoke<string>("get_platform").then((p) => setIsWindows(p === "windows"));
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
        {/* Header — hidden on Windows (title bar already shows product name) */}
        {!isWindows && (
          <div className={styles.header} data-tauri-drag-region>
            {!sidebarCollapsed && <h1 className={styles.title} data-tauri-drag-region>{t("title")}</h1>}
          </div>
        )}

        {/* Navigation */}
        <nav className={`${styles.nav}${sidebarCollapsed ? ` ${styles.nav_collapsed}` : ""}`} data-wizard="view-toggle">
          <button
            className={`${styles.nav_item} ${viewMode === "tasks" ? styles.nav_active : ""}`}
            onClick={() => setViewMode("tasks")}
          >
            <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M2.5 3.5h11v3h-11zM2.5 9.5h7v3h-7zM11 9.5l2.5 1.5L11 12.5z"/></svg></span>
            <span className={styles.nav_label}>{t("view_tasks", "Tasks")}</span>
          </button>
          <div className={styles.nav_project_row}>
            <button
              className={`${styles.nav_item} ${styles.nav_item_with_chevron} ${viewMode === "projects" ? styles.nav_active : ""}`}
              onClick={() => setViewMode("projects")}
            >
              <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M2 4.5h4.5l1.5 1.5h6V12a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 12V4.5Z"/></svg></span>
              <span className={styles.nav_label}>{t("view_projects")}</span>
            </button>
            {!sidebarCollapsed && projects.length > 0 && (
              <button
                type="button"
                className={styles.nav_chevron}
                onClick={toggleProjectsExpanded}
                title={projectsExpanded ? t("sidebar.projects_collapse") : t("sidebar.projects_expand")}
                aria-label={projectsExpanded ? t("sidebar.projects_collapse") : t("sidebar.projects_expand")}
                aria-expanded={projectsExpanded}
              >
                <svg
                  width="10"
                  height="10"
                  viewBox="0 0 10 10"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  style={{
                    transform: projectsExpanded ? "rotate(90deg)" : "rotate(0deg)",
                    transition: "transform 0.15s",
                  }}
                >
                  <polyline points="3,2 7,5 3,8" />
                </svg>
              </button>
            )}
          </div>
          {!sidebarCollapsed && projectsExpanded && projects.length > 0 && (
            <ul className={styles.nav_project_list}>
              {projects.map((p) => {
                const total = projectCounts.total.get(p.id) ?? 0;
                const running = projectCounts.running.get(p.id) ?? 0;
                return (
                  <li key={p.id}>
                    <button
                      type="button"
                      className={styles.nav_project_item}
                      onClick={() => openProject(p.id)}
                      title={p.name}
                    >
                      <span className={styles.nav_project_name}>{p.name}</span>
                      {running > 0 && (
                        <span className={`${styles.nav_project_chip} ${styles.nav_project_chip_running}`} title={t("active")}>
                          {running}
                        </span>
                      )}
                      <span className={styles.nav_project_chip} title={t("recent")}>
                        {total}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          <button
            className={styles.nav_item}
            onClick={() => setLauncherInitial({})}
            title={t("launcher.title")}
          >
            <span className={styles.nav_icon}>+</span>
            <span className={styles.nav_label}>{t("launcher.new_session")}</span>
          </button>
          <div className={styles.nav_divider} />
          <button
            className={`${styles.nav_item} ${viewMode === "list" ? styles.nav_active : ""}`}
            onClick={() => setViewMode("list")}
          >
            <span className={styles.nav_icon}>☰</span>
            <span className={styles.nav_label}>{t("view_list")}</span>
          </button>
          <button
            className={`${styles.nav_item} ${viewMode === "gallery" ? styles.nav_active : ""}`}
            onClick={() => setViewMode("gallery")}
          >
            <span className={styles.nav_icon}>⊞</span>
            <span className={styles.nav_label}>{t("view_gallery")}</span>
          </button>
          <button
            className={`${styles.nav_item} ${viewMode === "audit" ? styles.nav_active : ""}`}
            onClick={() => setViewMode("audit")}
          >
            <span className={styles.nav_icon}>⛨</span>
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
        </nav>

        <div className={styles.separator} />

        {/* Scrollable sidebar content */}
        <div className={styles.sidebar_content}>
          {/* Token Speed Chart */}
          <div data-wizard="token-speed">
            <TokenSpeedChart collapsed={sidebarCollapsed} />
          </div>

          {/* Cost Speed Chart (collapsed by default) — hidden in narrow rail */}
          {!sidebarCollapsed && <CostSpeedChart />}

          {/* Usage panel */}
          <UsagePanel collapsed={sidebarCollapsed} />

          {/* Mascot — hidden in narrow rail */}
          {!sidebarCollapsed && (
            <MascotEyes
              dashboardMode
              usageRing={usageRing ? {
                percent: usageRing.overall,
                topSource: usageRing.topSource,
                sources: usageRing.sources,
              } : null}
            />
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
                  {/* Dock-to-right-strip glyph — distinct from the mobile phone icon */}
                  <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
                    <rect x="1.5" y="2.5" width="13" height="11" rx="1.3" />
                    <rect x="10" y="2.5" width="4.5" height="11" rx="1.3" fill="currentColor" fillOpacity="0.35" stroke="none" />
                    <line x1="10" y1="2.5" x2="10" y2="13.5" />
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

        {/* Collapse / expand toggle */}
        <button
          type="button"
          className={styles.collapse_toggle}
          onClick={() => setSidebarCollapsed(!sidebarCollapsed)}
          title={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
          aria-label={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
        >
          <span className={styles.collapse_toggle_glyph}>{sidebarCollapsed ? "»" : "«"}</span>
        </button>

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
                  <p className={styles.empty}>{scanReady ? t("no_sessions") : t("scanning")}</p>
                )}
              </>
            ) : (
              <>
                {active.length > 0 && renderGroup(active)}
                {active.length === 0 && (
                  <p className={styles.empty}>{scanReady ? t("no_sessions") : t("scanning")}</p>
                )}
              </>
            )}
          </div>
        </div>
      ) : viewMode === "gallery" ? (
        <GalleryView />
      ) : viewMode === "audit" ? (
        <AuditView />
      ) : viewMode === "memory" ? (
        <MemoryView />
      ) : viewMode === "skills" ? (
        <SkillsView />
      ) : viewMode === "plugins" ? (
        <PluginsView />
      ) : viewMode === "projects" ? (
        <ProjectsView />
      ) : viewMode === "tasks" ? (
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
