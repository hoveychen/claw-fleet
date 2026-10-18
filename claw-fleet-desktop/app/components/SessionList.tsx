import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { FileText, Folder, Settings, SquareCheckBig } from "lucide-react";
import { openSettings, runningProcTotal, useAuditStore, useProcStore, useReportStore, useSessionsStore, useUIStore } from "../store";
import type { SessionInfo } from "../types";
import type { SessionsFrame } from "../../../shared-ts/sessionsFrame";
import { MemoryView } from "./MemoryView";
import { WikiView } from "./WikiView";
import { ArtifactsView } from "./ArtifactsView";
import { AuditView } from "./AuditView";
import { ScheduleView } from "./ScheduleView";
import { ReportView } from "./report/ReportView";
import { SkillsView } from "./SkillsView";
import { FilesView } from "./FilesView";
import { TerminalView } from "./TerminalView";
import { PluginsView } from "./PluginsView";
import { MobileView } from "./MobileView";
import { HistoryView } from "./HistoryView";
import { LobsterFleetView } from "./lobster/LobsterFleetView";
import { TopProgress } from "./loading";
import styles from "./SessionList.module.css";
import { TodayUsageBadge } from "./TodayUsageBadge";
import { UsagePanel } from "./UsagePanel";
import { SECONDARY_SIDEBAR_VIEWS } from "./pageShellConfig";
import { navSectionOf, navSectionTarget, type NavSection } from "./navGroups";
import { useNavAvailability } from "../hooks/useNavAvailability";
import { fmtBadgeCount } from "../railNumbers";

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
 * The rail is a fixed icon column with exactly four entries — Tasks (home),
 * Library, Projects, Settings — each a section of pages (see navGroups.ts). A
 * section with several pages switches between them from the page banner, so
 * the rail never grows a second level. Settings opens the Settings overlay;
 * its pages (audit, report, skills, phone) are reached from there.
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

  const current = navSectionOf(viewMode);
  // Re-clicking the entry of the page on screen folds that page's secondary
  // sidebar instead of being a no-op.
  const goSection = useCallback(
    (section: NavSection) => {
      const target = navSectionTarget(section, navLastBySection, avail);
      if (viewMode === target && SECONDARY_SIDEBAR_VIEWS.has(target)) {
        toggleSecondarySidebar(target);
      } else {
        setViewMode(target);
      }
    },
    [viewMode, navLastBySection, avail, setViewMode, toggleSecondarySidebar],
  );

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
    // Keep the running-command total fresh for the repos nav badge even when the
    // files view isn't open (FilesView also polls, but only while mounted).
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
      <aside className={styles.sidebar}>
        {/* Drag strip under the macOS traffic lights. */}
        <div className={styles.header} data-tauri-drag-region />

        <nav className={styles.nav} data-wizard="view-toggle">
          <NavEntry
            icon={<SquareCheckBig size={18} strokeWidth={1.6} />}
            label={t("nav_section_tasks", "任务")}
            active={!settingsOpen && current === "tasks"}
            onClick={() => goSection("tasks")}
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
          <NavEntry
            icon={<Settings size={18} strokeWidth={1.6} />}
            label={t("nav_section_settings", "设置")}
            active={settingsOpen || current === "settings"}
            onClick={openSettings}
            // An unread critical audit event or a fresh daily report used to
            // badge its own nav item; those pages now live under Settings, so
            // the nudge rides on its entry — a dot, since a red count reads as
            // an error rather than a nudge.
            badge={(unreadCriticalCount > 0 || hasNewReport) && <span className={styles.nav_dot} />}
          />
        </nav>

        <div className={styles.sidebar_content} />

        {/* Global usage tiles stay on every page. */}
        <div className={styles.rail_tiles} data-wizard="settings-footer">
          <TodayUsageBadge collapsed />
          <UsagePanel collapsed />
        </div>
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
      ) : viewMode === "schedule" ? (
        <ScheduleView />
      ) : viewMode === "wiki" ? (
        <WikiView />
      ) : viewMode === "artifacts" ? (
        <ArtifactsView />
      ) : viewMode === "skills" ? (
        <SkillsView />
      ) : viewMode === "files" ? (
        <FilesView />
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
