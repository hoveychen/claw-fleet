import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Coffee, FileText, Folder, History, Monitor, Moon, PanelLeftClose, PanelLeftOpen, Settings, SquareCheckBig, Sun } from "lucide-react";
import { openSettings, runningProcTotal, useAuditStore, useDecisionStore, useProcStore, useReportStore, useSessionsStore, useUIStore } from "../store";
import type { SessionInfo } from "../types";
import type { SessionsFrame } from "../../../shared-ts/sessionsFrame";
import { MemoryView } from "./MemoryView";
import { WikiView } from "./WikiView";
import { LibraryView } from "./LibraryView";
import { ArtifactsView } from "./ArtifactsView";
import { AuditView } from "./AuditView";
import { ProjectsView } from "./ProjectsView";
import { ReportView } from "./report/ReportView";
import { SkillsView } from "./SkillsView";
import { TerminalView } from "./TerminalView";
import { PluginsView } from "./PluginsView";
import { MobileView } from "./MobileView";
import { HistoryView } from "./HistoryView";
import { LobsterFleetView } from "./lobster/LobsterFleetView";
import { TopProgress } from "./loading";
import styles from "./SessionList.module.css";
import { TodayUsageBadge } from "./TodayUsageBadge";
import { UsagePanel } from "./UsagePanel";
import { LiveStats } from "./LiveStats";
import { ResizeHandle } from "./ResizeHandle";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { useKeepAwake } from "../hooks/useKeepAwake";
import { SECONDARY_SIDEBAR_VIEWS } from "./pageShellConfig";
import { GLOBAL_MEMORY_FILTER, navSectionOfPage, navSectionTarget, type NavSection } from "./navGroups";
import { useNavAvailability } from "../hooks/useNavAvailability";
import { fmtBadgeCount } from "../railNumbers";

const RAIL_WIDTH = 64;
const MIN_WIDTH = 200;
const MAX_WIDTH = 400;
const DEFAULT_WIDTH = 232;

function NavEntry({
  icon,
  label,
  active,
  onClick,
  badge,
}: {
  icon: ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
  badge?: ReactNode;
}) {
  return (
    <button
      type="button"
      className={`${styles.nav_item}${active ? ` ${styles.nav_active}` : ""}`}
      onClick={onClick}
      aria-current={active ? "page" : undefined}
    >
      <span className={styles.nav_icon}>{icon}</span>
      <span className={styles.nav_label}>{label}</span>
      {badge}
    </button>
  );
}

/**
 * The app's left rail and page switch.
 *
 * The nav has exactly four entries — Tasks (home), Library, Projects, Settings
 * (plus Review) — each a section of pages (see navGroups.ts). A section with
 * several pages switches between them from the page banner, so the nav never
 * grows a second level. Settings opens the Settings overlay; its pages (audit,
 * report, skills, phone) are reached from there.
 *
 * Two widths. Folded (the default) it is a 64px icon column whose foot holds
 * bare number tiles. Unfolded it is a drag-resizable sidebar: icon + label
 * rows, the live token-speed / spend-rate block with its 5-minute sparklines,
 * the usage panel inline, and a toolbar with keep-awake, theme and fold.
 */
export function SessionList() {
  const { t } = useTranslation();
  const { refresh, setSessions, applyFrame, setScanReady } = useSessionsStore();
  const viewMode = useUIStore((s) => s.viewMode);
  const setViewMode = useUIStore((s) => s.setViewMode);
  const toggleSecondarySidebar = useUIStore((s) => s.toggleSecondarySidebar);
  const navLastBySection = useUIStore((s) => s.navLastBySection);
  const settingsOpen = useUIStore((s) => s.settingsOpen);
  const terminalEnabled = useUIStore((s) => s.hostFeatures.terminal);
  const avail = useNavAvailability();
  const unreadCriticalCount = useAuditStore((s) => s.unreadCriticalCount);
  const hasNewReport = useReportStore((s) => s.hasNewReport);
  // Running workspace commands across all repos, badged on Projects (its
  // first page is the code browser that lists them).
  const runningProcCount = useProcStore((s) => runningProcTotal(s.procs));
  // Cards waiting on an answer, badged on Tasks: the card is answered in the
  // task's conversation, so this is how it is noticed from any other page.
  const pendingCardCount = useDecisionStore((s) => s.decisions.length);

  const collapsed = useUIStore((s) => s.sidebarCollapsed);
  const setCollapsed = useUIStore((s) => s.setSidebarCollapsed);
  const theme = useUIStore((s) => s.theme);
  const setTheme = useUIStore((s) => s.setTheme);
  const { enabled: keepAwake, supported: keepAwakeSupported, setKeepAwake } = useKeepAwake();
  const {
    width: wideWidth,
    isDragging,
    onMouseDown: onResizeMouseDown,
  } = useResizableWidth("sidebar-width", { min: MIN_WIDTH, max: MAX_WIDTH, initial: DEFAULT_WIDTH });
  // Folding re-lays the sidebar out in one frame (labels drop, the panels turn
  // into tiles) while the width is still easing; fading the new layout in hides
  // the jump. The class differs per direction so every toggle restarts the
  // animation, and it is null until the first toggle so launch doesn't fade.
  const settle = useRef<"collapsed" | "expanded" | null>(null);
  const prevCollapsed = useRef(collapsed);
  if (prevCollapsed.current !== collapsed) {
    prevCollapsed.current = collapsed;
    settle.current = collapsed ? "collapsed" : "expanded";
  }

  const memoryFilter = useUIStore((s) => s.mainViewState.memory.workspaceFilter);
  const current = navSectionOfPage(viewMode, memoryFilter);
  // Re-clicking the entry of the page on screen folds that page's secondary
  // sidebar instead of being a no-op.
  const goSection = useCallback(
    (section: NavSection) => {
      const target = navSectionTarget(section, navLastBySection, avail);
      // Projects' memory page is project memory; global memory is a Settings page.
      if (target === "memory" && memoryFilter === GLOBAL_MEMORY_FILTER) {
        useUIStore.getState().updateMainViewState("memory", { workspaceFilter: "all", selectedKey: null });
      }
      if (viewMode === target && SECONDARY_SIDEBAR_VIEWS.has(target)) {
        toggleSecondarySidebar(target);
      } else {
        setViewMode(target);
      }
    },
    [viewMode, navLastBySection, avail, memoryFilter, setViewMode, toggleSecondarySidebar],
  );

  // ⌘N / Ctrl+N anywhere: back to the Tasks page with the new-task composer open.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "n") {
        e.preventDefault();
        useUIStore.getState().requestLaunchFocus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    // Load audit data for the unread critical badge
    invoke<import("../types").AuditSummary>("get_audit_events")
      .then((data) => {
        useAuditStore.getState().setCriticalEvents(
          data.events.filter((e) => e.riskLevel === "critical")
        );
      })
      .catch(() => {});
    // Compute the "new report" red dot without opening the report view.
    useReportStore.getState().refreshNewReportFlag();
    // Register event listeners BEFORE calling refresh() to avoid a race
    // condition: on Linux the initial background scan can complete so fast
    // that the "sessions-updated" event fires before the listener is set up,
    // causing existing sessions to be invisible until a new one is created.
    const unlistenFrames = listen<SessionsFrame<SessionInfo>>("sessions-frame", (e) => {
      applyFrame(e.payload);
    });
    // The browser build's poller and the mock still push whole, unnumbered lists.
    const unlistenPromise = listen<SessionInfo[]>("sessions-updated", (e) => {
      setSessions(e.payload);
    });
    const unlistenScanReady = listen<boolean>("scan-ready", () => {
      setScanReady(true);
    });
    // Refresh after listeners are registered. Even if the initial scan event
    // was already emitted, this fetch will pick up whatever has been scanned
    // so far; and any future events will be caught by the listeners above.
    Promise.all([unlistenFrames, unlistenPromise]).then(() => refresh());
    unlistenScanReady.then(() => refresh());
    // Keep the workspace-command list fresh app-wide: the Projects page's row
    // badges and code tab read it, and so does the nav badge.
    const fetchProcs = useProcStore.getState().fetchProcs;
    fetchProcs();
    const procTimer = setInterval(fetchProcs, 2000);
    return () => {
      unlistenFrames.then((u) => u());
      unlistenPromise.then((u) => u());
      unlistenScanReady.then((u) => u());
      clearInterval(procTimer);
    };
  }, []);

  return (
    <>
      <aside
        className={`${styles.sidebar}${collapsed ? "" : ` ${styles.sidebar_wide}`}${
          settle.current === "collapsed"
            ? ` ${styles.settle_collapsed}`
            : settle.current === "expanded"
              ? ` ${styles.settle_expanded}`
              : ""
        }${isDragging ? ` ${styles.sidebar_dragging}` : ""}`}
        style={{ width: collapsed ? RAIL_WIDTH : wideWidth }}
      >
        {/* Drag strip under the macOS traffic lights. */}
        <div className={styles.header} data-tauri-drag-region />

        <nav className={styles.nav} data-wizard="view-toggle">
          <NavEntry
            icon={<SquareCheckBig size={18} strokeWidth={1.6} />}
            label={t("nav_section_tasks", "任务")}
            active={!settingsOpen && current === "tasks"}
            onClick={() => goSection("tasks")}
            badge={
              pendingCardCount > 0 && (
                <span className={styles.nav_badge_pending} title={`${pendingCardCount}`}>
                  {fmtBadgeCount(pendingCardCount)}
                </span>
              )
            }
          />
          <NavEntry
            icon={<FileText size={18} strokeWidth={1.6} />}
            label={t("nav_section_library", "资料")}
            active={!settingsOpen && current === "library"}
            onClick={() => goSection("library")}
          />
          <NavEntry
            icon={<Folder size={18} strokeWidth={1.6} />}
            label={t("nav_section_projects", "项目")}
            active={!settingsOpen && current === "projects"}
            onClick={() => goSection("projects")}
            badge={
              runningProcCount > 0 && (
                <span className={styles.nav_badge_running} title={`${runningProcCount}`}>
                  {fmtBadgeCount(runningProcCount)}
                </span>
              )
            }
          />
          {/* Secondary entry beside Settings: the daily review. A fresh one
              dots it, as the daily report's entry used to. */}
          <NavEntry
            icon={<History size={18} strokeWidth={1.6} />}
            label={t("nav_section_review", "回顾")}
            active={!settingsOpen && current === "review"}
            onClick={() => goSection("review")}
            badge={hasNewReport && <span className={styles.nav_dot} />}
          />
          <NavEntry
            icon={<Settings size={18} strokeWidth={1.6} />}
            label={t("nav_section_settings", "设置")}
            active={settingsOpen || current === "settings"}
            onClick={openSettings}
            // An unread critical audit event used to badge its own nav item;
            // that page now lives under Settings, so the nudge rides on its
            // entry — a dot, since a red count reads as an error rather than
            // a nudge.
            badge={unreadCriticalCount > 0 && <span className={styles.nav_dot} />}
          />
        </nav>

        {collapsed ? (
          <>
            <div className={styles.sidebar_content} />
            {/* Global usage tiles stay on every page. */}
            <div className={styles.rail_tiles} data-wizard="settings-footer">
              {/* On a short window the nav keeps its room and these tiles give
                  way first; see .rail_tile_live / .rail_tile_today. */}
              <div className={styles.rail_tile_today}>
                <TodayUsageBadge collapsed />
              </div>
              <div className={styles.rail_tile_live}>
                <LiveStats collapsed />
              </div>
              <UsagePanel collapsed />
              <button
                type="button"
                className={styles.rail_expand}
                onClick={() => setCollapsed(false)}
                title={t("sidebar.expand")}
                aria-label={t("sidebar.expand")}
              >
                <PanelLeftOpen size={15} strokeWidth={1.6} />
              </button>
            </div>
          </>
        ) : (
          <>
            {/* Wide, the usage panel opens inline here instead of as the
                popover the rail tile raises. */}
            <div className={styles.sidebar_content}>
              <TodayUsageBadge />
              <LiveStats />
              <UsagePanel />
            </div>
            <div className={styles.footer} data-wizard="settings-footer">
              <div className={styles.footer_toolbar} role="group">
                {keepAwakeSupported && (
                  <button
                    type="button"
                    className={`${styles.footer_btn}${keepAwake ? ` ${styles.footer_btn_active}` : ""}`}
                    onClick={() => setKeepAwake(!keepAwake)}
                    title={keepAwake ? t("keep_awake_on_tooltip") : t("keep_awake_off_tooltip")}
                    aria-label={keepAwake ? t("keep_awake_on_tooltip") : t("keep_awake_off_tooltip")}
                    aria-pressed={keepAwake}
                  >
                    <Coffee size={14} strokeWidth={1.6} />
                  </button>
                )}
                <button
                  type="button"
                  className={styles.footer_btn}
                  // Cycle light → dark → system → light.
                  onClick={() => setTheme(theme === "light" ? "dark" : theme === "dark" ? "system" : "light")}
                  title={`${t("theme.cycle")}: ${t(`theme.${theme}`)}`}
                  aria-label={`${t("theme.cycle")}: ${t(`theme.${theme}`)}`}
                >
                  {theme === "light" ? (
                    <Sun size={14} strokeWidth={1.6} />
                  ) : theme === "dark" ? (
                    <Moon size={14} strokeWidth={1.6} />
                  ) : (
                    <Monitor size={14} strokeWidth={1.6} />
                  )}
                </button>
                <button
                  type="button"
                  className={styles.footer_btn}
                  onClick={() => setCollapsed(true)}
                  title={t("sidebar.collapse")}
                  aria-label={t("sidebar.collapse")}
                >
                  <PanelLeftClose size={14} strokeWidth={1.6} />
                </button>
              </div>
            </div>
            <ResizeHandle active={isDragging} onMouseDown={onResizeMouseDown} />
          </>
        )}
      </aside>

      {/* Main content area */}
      {viewMode === "history" ? (
        <HistoryView />
      ) : viewMode === "lobsters" ? (
        <LobsterFleetView />
      ) : viewMode === "audit" ? (
        <AuditView />
      ) : viewMode === "memory" ? (
        <MemoryView />
      ) : viewMode === "projects" ? (
        <ProjectsView />
      ) : viewMode === "library" ? (
        <LibraryView />
      ) : viewMode === "wiki" ? (
        <WikiView />
      ) : viewMode === "artifacts" ? (
        <ArtifactsView />
      ) : viewMode === "skills" ? (
        <SkillsView />
      ) : viewMode === "terminal" ? (
        // `loadHostFeatures` navigates a restored `terminal` viewMode back home
        // when the flag is off; this guard covers the frame before that answer
        // lands, so the page never flashes a shell the backend would refuse.
        // That answer always lands (a failed probe fails closed and navigates
        // home), so "off while on this page" can only mean "not answered yet":
        // show the progress bar rather than a blank main area.
        terminalEnabled ? (
          <TerminalView />
        ) : (
          <div className={styles.main_pending}>
            <TopProgress active />
          </div>
        )
      ) : viewMode === "plugins" ? (
        <PluginsView />
      ) : viewMode === "mobile" ? (
        <MobileView />
      ) : (
        <ReportView />
      )}

    </>
  );
}
