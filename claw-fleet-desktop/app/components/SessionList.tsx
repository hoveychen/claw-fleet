import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, type ReactNode } from "react";
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
import { useKeepAwake } from "../hooks/useKeepAwake";
import { hasFoldableListColumn } from "./pageShellConfig";
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
 * The nav has exactly four entries — Tasks (home), Library, Projects, Settings
 * (plus Review) — each a section of pages (see navGroups.ts). A section with
 * several pages switches between them from the page banner, so the nav never
 * grows a second level. Settings opens the Settings overlay; its pages (audit,
 * report, skills, phone) are reached from there.
 *
 * One fixed 116px width (not drag-resizable: at the old 232px default the
 * sidebar took a fifth of a 13" window for a handful of entries and numbers):
 * icon + label rows, a stack of stat cards (today's spend, token speed and
 * spend rate with their 5-minute sparklines, usage per source — the full usage
 * panel opens as a popover), and a toolbar with keep-awake, theme and a toggle
 * for the current page's list column (its left-hand secondary sidebar). The
 * nav itself does not fold.
 */
export function SessionList() {
  const { t } = useTranslation();
  const { refresh, setSessions, applyFrame, setScanReady } = useSessionsStore();
  const viewMode = useUIStore((s) => s.viewMode);
  const setViewMode = useUIStore((s) => s.setViewMode);
  const toggleSecondarySidebar = useUIStore((s) => s.toggleSecondarySidebar);
  const secondaryCollapsed = useUIStore((s) => !!s.secondarySidebarCollapsed[viewMode]);
  const navLastBySection = useUIStore((s) => s.navLastBySection);
  const settingsOpen = useUIStore((s) => s.settingsOpen);
  // Simplified mode (a Settings switch) drops Projects, Review and the
  // token-speed / usage blocks from the sidebar.
  const simplified = useUIStore((s) => s.simplifiedMode);
  const terminalEnabled = useUIStore((s) => s.hostFeatures.terminal);
  const avail = useNavAvailability();
  const hasNewReport = useReportStore((s) => s.hasNewReport);
  // Running workspace commands across all repos, badged on Projects (its
  // first page is the code browser that lists them).
  const runningProcCount = useProcStore((s) => runningProcTotal(s.procs));
  // Cards waiting on an answer, badged on Tasks: the card is answered in the
  // task's conversation, so this is how it is noticed from any other page.
  const pendingCardCount = useDecisionStore((s) => s.decisions.length);

  const theme = useUIStore((s) => s.theme);
  const setTheme = useUIStore((s) => s.setTheme);
  const { enabled: keepAwake, supported: keepAwakeSupported, setKeepAwake } = useKeepAwake();
  // The footer toggle folds the page's list column.
  const hasListColumn = hasFoldableListColumn(viewMode);

  const current = navSectionOf(viewMode);
  // Re-clicking the entry of the page on screen folds that page's secondary
  // sidebar instead of being a no-op — the same toggle, behind the same gate,
  // as the footer fold button.
  const goSection = useCallback(
    (section: NavSection) => {
      const target = navSectionTarget(section, navLastBySection, avail);
      if (viewMode === target && hasFoldableListColumn(target)) {
        toggleSecondarySidebar(target);
      } else {
        setViewMode(target);
      }
    },
    [viewMode, navLastBySection, avail, setViewMode, toggleSecondarySidebar],
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
      <aside className={styles.sidebar}>
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
          {!simplified && (
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
          )}
          {/* Secondary entry beside Settings: the daily review. A fresh one
              dots it, as the daily report's entry used to. */}
          {!simplified && (
            <NavEntry
              icon={<History size={18} strokeWidth={1.6} />}
              label={t("nav_section_review", "回顾")}
              active={!settingsOpen && current === "review"}
              onClick={() => goSection("review")}
              badge={hasNewReport && <span className={styles.nav_dot} />}
            />
          )}
          <NavEntry
            icon={<Settings size={18} strokeWidth={1.6} />}
            label={t("nav_section_settings", "设置")}
            active={settingsOpen || current === "settings"}
            onClick={openSettings}
            // No badge here: unread critical audit events are dotted on the
            // Security tab inside the Settings panel instead.
          />
        </nav>

        <div className={styles.sidebar_content}>
          {!simplified && (
            <>
              <TodayUsageBadge />
              <LiveStats />
              <UsagePanel />
            </>
          )}
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
              // Lit while the list is folded: with no strip left on the page,
              // this button is the visible way back.
              className={`${styles.footer_btn}${
                hasListColumn && secondaryCollapsed ? ` ${styles.footer_btn_active}` : ""
              }`}
              onClick={() => toggleSecondarySidebar(viewMode)}
              disabled={!hasListColumn}
              aria-pressed={hasListColumn ? secondaryCollapsed : undefined}
              title={secondaryCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
              aria-label={secondaryCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
            >
              {hasListColumn && secondaryCollapsed ? (
                <PanelLeftOpen size={14} strokeWidth={1.6} />
              ) : (
                <PanelLeftClose size={14} strokeWidth={1.6} />
              )}
            </button>
          </div>
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
