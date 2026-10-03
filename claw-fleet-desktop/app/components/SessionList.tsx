import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Shield, ListChecks, Coffee, ListTree, Package, SquareTerminal, Ellipsis, ChevronLeft, ChevronRight } from "lucide-react";
import { useKeepAwake } from "../hooks/useKeepAwake";
import { openSettings, runningProcTotal, useAuditStore, useProcStore, useReportStore, useSessionsStore, useUIStore } from "../store";
import type { ViewMode } from "../store";
import { isWebBuild, showsMobilePanel } from "../hostEnv";
import type { SessionInfo } from "../types";
import type { SessionsFrame } from "../../../shared-ts/sessionsFrame";
import { MascotEyes } from "./MascotEyes";
import { useUsageRing } from "../hooks/useUsageRing";
import { MemoryView } from "./MemoryView";
import { WikiView } from "./WikiView";
import { ArtifactsView } from "./ArtifactsView";
import { AuditView } from "./AuditView";
import { ScheduleView } from "./ScheduleView";
import { PlansView } from "./PlansView";
import { ReportView } from "./report/ReportView";
import { SkillsView } from "./SkillsView";
import { FilesView } from "./FilesView";
import { TerminalView } from "./TerminalView";
import { PluginsView } from "./PluginsView";
import { MobileView } from "./MobileView";
import { HistoryView } from "./HistoryView";
import { TopProgress } from "./loading";
import styles from "./SessionList.module.css";
import { LiveStats } from "./LiveStats";
import { TodayUsageBadge } from "./TodayUsageBadge";
import { UsagePanel } from "./UsagePanel";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { useHeightTransition } from "../hooks/useHeightTransition";
import { ResizeHandle } from "./ResizeHandle";
import { SECONDARY_SIDEBAR_VIEWS } from "./pageShellConfig";
import { isInNavMore } from "./navGroups";
import { fmtBadgeCount } from "../railNumbers";

const MIN_WIDTH = 200;
const MAX_WIDTH = 520;
const DEFAULT_WIDTH = 280;


export function SessionList() {
  const { t } = useTranslation();
  const { refresh, setSessions, applyFrame, setScanReady } = useSessionsStore();
  const {
    simplifiedMode,
    viewMode,
    setViewMode,
    theme,
    setTheme,
    sidebarCollapsed,
    setSidebarCollapsed,
    toggleSecondarySidebar,
    mascotVisible,
  } = useUIStore();
  // Terminal page only exists if backend started with FLEET_TERMINAL (see core's feature_flags).
  // When off, not even the nav item shows, rather than letting users click in and get rejected by backend.
  const terminalEnabled = useUIStore((s) => s.hostFeatures.terminal);
  const { enabled: keepAwake, supported: keepAwakeSupported, setKeepAwake } = useKeepAwake();
  // Views that own a secondary sidebar (two-level sidebar). Re-clicking the nav item of
  // the already-active one collapses/expands its sidebar instead of being a
  // no-op; every other view just switches as usual.
  const navTo = useCallback(
    (target: ViewMode) => {
      if (viewMode === target && SECONDARY_SIDEBAR_VIEWS.has(target)) {
        toggleSecondarySidebar(target);
      } else {
        setViewMode(target);
      }
    },
    [viewMode, setViewMode, toggleSecondarySidebar],
  );
  const unreadCriticalCount = useAuditStore((s) => s.unreadCriticalCount);
  const hasNewReport = useReportStore((s) => s.hasNewReport);
  // Total running workspace commands across all repos — surfaced as a badge on
  // the Files nav item, mirroring the green per-repo badge in FilesView.
  const runningProcCount = useProcStore((s) => runningProcTotal(s.procs));
  // The less-frequent pages live on a "More" sub-page of the nav: clicking More
  // swaps the whole list for its items under a back row. The nav follows the
  // page on screen — it enters More whenever one of its pages comes up and
  // leaves it when the page moves back to the top level, including hops that
  // bypass the nav (an audit link, a tray click into Tasks).
  const moreActive = isInNavMore(viewMode);
  const [inMore, setInMore] = useState(moreActive);
  useEffect(() => {
    setInMore(moreActive);
  }, [moreActive]);
  // The swap slides in from the side it came from: More drills in from the
  // right, Back returns from the left. Null until the first swap, so the list
  // the app opens on doesn't animate in. Derived during render, not in an
  // effect: an effect would paint the new list in place for one frame first.
  const navSlide = useRef<"forward" | "back" | null>(null);
  const prevInMore = useRef(inMore);
  if (prevInMore.current !== inMore) {
    prevInMore.current = inMore;
    navSlide.current = inMore ? "forward" : "back";
  }
  // The two lists differ in length; without this, everything under the nav
  // jumps the moment the list swaps.
  const navRef = useRef<HTMLElement>(null);
  useHeightTransition(navRef, inMore, 200);
  // On the top level the More items' badges are out of sight, which is how an
  // unread critical audit event goes unnoticed for an hour. Roll them up onto
  // the More entry as one quiet dot — a red count there reads as an error, not
  // a nudge; the exact count still sits on the Audit item inside.
  const moreDot = hasNewReport || unreadCriticalCount > 0;
  const {
    width: sidebarWidth,
    isDragging,
    onMouseDown: handleResizeMouseDown,
  } = useResizableWidth("sidebar-width", {
    min: MIN_WIDTH,
    max: MAX_WIDTH,
    initial: DEFAULT_WIDTH,
  });
  const usageRing = useUsageRing();

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

  const COLLAPSED_WIDTH = 64;
  const effectiveWidth = sidebarCollapsed ? COLLAPSED_WIDTH : sidebarWidth;
  // Collapsing re-lays the whole sidebar out in one frame (labels drop, icons
  // re-centre, the panels switch to rail tiles) while the width is still
  // easing. Fading the new layout in hides that jump. The class name differs
  // per direction so each toggle restarts the animation; null until the first
  // toggle, so the app doesn't fade its sidebar in on launch.
  const sidebarSettle = useRef<"collapsed" | "expanded" | null>(null);
  const prevCollapsed = useRef(sidebarCollapsed);
  if (prevCollapsed.current !== sidebarCollapsed) {
    prevCollapsed.current = sidebarCollapsed;
    sidebarSettle.current = sidebarCollapsed ? "collapsed" : "expanded";
  }

  // The items on the "More" sub-page.
  const moreItems = (
    <>
      <button
        className={`${styles.nav_item} ${viewMode === "schedule" ? styles.nav_active : ""}`}
        onClick={() => navTo("schedule")}
      >
        <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><rect x="2.5" y="3" width="11" height="10.5" rx="1.5"/><path d="M2.5 6.5H13.5"/><path d="M5.5 1.5V3.5"/><path d="M10.5 1.5V3.5"/><path d="M8 8.5v2l1.3.8"/></svg></span>
        <span className={styles.nav_label}>{t("view_schedule", "计划")}</span>
      </button>
      <button
        className={`${styles.nav_item} ${viewMode === "plans" ? styles.nav_active : ""}`}
        onClick={() => navTo("plans")}
      >
        <span className={styles.nav_icon}><ListTree size={14} strokeWidth={1.5} /></span>
        <span className={styles.nav_label}>{t("view_plans", "计划树")}</span>
      </button>

      <div className={styles.nav_divider} />

      <button
        className={`${styles.nav_item} ${viewMode === "audit" ? styles.nav_active : ""}`}
        onClick={() => navTo("audit")}
      >
        <span className={styles.nav_icon}><Shield size={14} strokeWidth={1.5} /></span>
        <span className={styles.nav_label}>{t("view_audit")}</span>
        {unreadCriticalCount > 0 && (
          <span className={styles.nav_badge} title={`${unreadCriticalCount}`}>
            {fmtBadgeCount(unreadCriticalCount)}
          </span>
        )}
      </button>
      <button
        className={`${styles.nav_item} ${viewMode === "report" ? styles.nav_active : ""}`}
        onClick={() => setViewMode("report")}
      >
        <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3"><rect x="1.5" y="2.5" width="13" height="12" rx="1.5"/><line x1="1.5" y1="5.5" x2="14.5" y2="5.5"/><line x1="5" y1="1" x2="5" y2="4"/><line x1="11" y1="1" x2="11" y2="4"/></svg></span>
        <span className={styles.nav_label}>{t("view_report")}</span>
        {hasNewReport && <span className={styles.nav_dot} />}
      </button>

      <button
        className={`${styles.nav_item} ${viewMode === "memory" ? styles.nav_active : ""}`}
        onClick={() => navTo("memory")}
      >
        <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M4 2.5h6.5a2 2 0 0 1 2 2v9a1 1 0 0 1-1 1H4a1.5 1.5 0 0 1-1.5-1.5V4A1.5 1.5 0 0 1 4 2.5Z"/><path d="M2.5 11.5H12"/><path d="M5.5 5.5h4"/><path d="M5.5 7.5h3"/></svg></span>
        <span className={styles.nav_label}>{t("view_memory")}</span>
      </button>
      <button
        className={`${styles.nav_item} ${viewMode === "skills" || viewMode === "plugins" ? styles.nav_active : ""}`}
        onClick={() => navTo(viewMode === "plugins" ? "plugins" : "skills")}
      >
        <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M9 1.5 3.5 9h4L7 14.5 12.5 7h-4L9 1.5Z"/></svg></span>
        <span className={styles.nav_label}>{t("view_skills")}</span>
      </button>
      {/* The pairing code belongs to "the process that opens the relay channel."
          Desktop always owns it. Cloud deployments run `fleet webui` on the same
          `hooks_server::serve`, which has `mobile_relay::ensure_ws_client()` — so that
          container also gets its own code; scan it and the device roster adds a cloud host.
          Local webui is the opposite: it only listens on loopback, behind it is no host phones
          can reach, the code only produces an unreachable device. See showsMobilePanel in hostEnv.ts. */}
      {showsMobilePanel(
        isWebBuild(),
        window.location.protocol,
        window.location.hostname,
      ) && (
        <button
          className={`${styles.nav_item} ${viewMode === "mobile" ? styles.nav_active : ""}`}
          onClick={() => navTo("mobile")}
        >
          <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><rect x="4" y="1.5" width="8" height="13" rx="1.6"/><line x1="7" y1="12.5" x2="9" y2="12.5"/></svg></span>
          <span className={styles.nav_label}>{t("view_mobile", "移动端")}</span>
        </button>
      )}
    </>
  );
  const workItems = (
    <>
      <button
        className={`${styles.nav_item} ${viewMode === "history" ? styles.nav_active : ""}`}
        onClick={() => navTo("history")}
      >
        <span className={styles.nav_icon}><ListChecks size={14} strokeWidth={1.5} /></span>
        <span className={styles.nav_label}>{t("view_history", "任务")}</span>
      </button>
      <button
        className={`${styles.nav_item} ${viewMode === "files" ? styles.nav_active : ""}`}
        onClick={() => navTo("files")}
      >
        <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M1.5 4a1.5 1.5 0 0 1 1.5-1.5h3L7.5 4H13a1.5 1.5 0 0 1 1.5 1.5v7A1.5 1.5 0 0 1 13 14H3a1.5 1.5 0 0 1-1.5-1.5V4Z"/></svg></span>
        <span className={styles.nav_label}>{t("view_files", "仓库")}</span>
        {runningProcCount > 0 && (
          <span className={styles.nav_badge_running} title={`${runningProcCount}`}>
            {fmtBadgeCount(runningProcCount)}
          </span>
        )}
      </button>
      {terminalEnabled && (
        <button
          className={`${styles.nav_item} ${viewMode === "terminal" ? styles.nav_active : ""}`}
          onClick={() => navTo("terminal")}
        >
          <span className={styles.nav_icon}><SquareTerminal size={14} strokeWidth={1.5} /></span>
          <span className={styles.nav_label}>{t("view_terminal", "终端")}</span>
        </button>
      )}
      <button
        className={`${styles.nav_item} ${viewMode === "wiki" ? styles.nav_active : ""}`}
        onClick={() => navTo("wiki")}
      >
        <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round"><path d="M8 3.5C6.8 2.4 5.2 2 3.5 2c-.6 0-1 .4-1 1v8.5c0 .6.4 1 1 1 1.7 0 3.3.4 4.5 1.5 1.2-1.1 2.8-1.5 4.5-1.5.6 0 1-.4 1-1V3c0-.6-.4-1-1-1-1.7 0-3.3.4-4.5 1.5Z"/><path d="M8 3.5V14"/></svg></span>
        <span className={styles.nav_label}>{t("view_wiki", "知识库")}</span>
      </button>
      <button
        className={`${styles.nav_item} ${viewMode === "artifacts" ? styles.nav_active : ""}`}
        onClick={() => navTo("artifacts")}
      >
        <span className={styles.nav_icon}><Package size={14} strokeWidth={1.5} /></span>
        <span className={styles.nav_label}>{t("view_artifacts", "产出")}</span>
      </button>

    </>
  );

  return (
    <>
      {!simplifiedMode && <aside
        className={`${styles.sidebar}${sidebarCollapsed ? ` ${styles.sidebar_collapsed}` : ""}${
          sidebarSettle.current === "collapsed" ? ` ${styles.sidebar_settle_collapsed}` : sidebarSettle.current === "expanded" ? ` ${styles.sidebar_settle_expanded}` : ""
        }`}
        style={{ width: effectiveWidth }}
      >
        {/* Empty header strip — reserves the top-right space for the collapse
            toggle and provides a drag region around the macOS traffic lights. */}
        <div className={styles.header} data-tauri-drag-region />

        {/* Collapse / expand toggle — absolute positioned top-right of the sidebar. */}
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

        {/* Sidebar nav: the work pages at the top level; schedules, plan trees
            and the monitoring / administration pages (audit, report, memory,
            skills, phone) on the "More" sub-page. Plugins are a source of
            skills, so they live under the Skills entry as a segmented tab
            (SkillsSourceTabs), not a separate nav item. The 64px rail uses the
            same two levels, its labels hidden. */}
        <nav ref={navRef} className={`${styles.nav}${sidebarCollapsed ? ` ${styles.nav_collapsed}` : ""}`} data-wizard="view-toggle">
          <div
            key={inMore ? "more" : "top"}
            className={`${styles.nav_pane}${
              navSlide.current === "forward" ? ` ${styles.nav_pane_forward}` : navSlide.current === "back" ? ` ${styles.nav_pane_back}` : ""
            }`}
          >
          {inMore ? (
            <>
              <button
                type="button"
                className={`${styles.nav_item} ${styles.nav_back}`}
                onClick={() => setInMore(false)}
                title={t("nav_back", "返回")}
                aria-label={t("nav_back", "返回")}
              >
                <span className={styles.nav_icon}><ChevronLeft size={14} strokeWidth={1.75} /></span>
                <span className={styles.nav_label}>{t("nav_more", "更多")}</span>
              </button>
              <div className={styles.nav_divider} />
              {moreItems}
            </>
          ) : (
            <>
              {workItems}
              <div className={styles.nav_divider} />
              <button
                type="button"
                className={`${styles.nav_item} ${moreActive ? styles.nav_active : ""}`}
                onClick={() => setInMore(true)}
                title={t("nav_more", "更多")}
              >
                <span className={styles.nav_icon}><Ellipsis size={14} strokeWidth={1.5} /></span>
                <span className={styles.nav_label}>{t("nav_more", "更多")}</span>
                {moreDot && <span className={styles.nav_dot} />}
                <span className={styles.nav_more_chevron}>
                  <ChevronRight size={12} strokeWidth={1.75} />
                </span>
              </button>
            </>
          )}
          </div>
        </nav>

        <div className={styles.separator} />

        {/* Scrollable sidebar content — charts + usage hidden for the
            task-focused views (projects / tasks) to keep that rail clean. */}
        <div className={styles.sidebar_content}>
          {/* Collapsed, the three panels degrade to bare number tiles. They
              share one column here so the stack keeps a single gap and a
              single edge inset — each panel used to bring its own margin and
              the rail read as three misaligned boxes. */}
          <div className={sidebarCollapsed ? styles.rail_tiles : undefined}>
            <TodayUsageBadge collapsed={sidebarCollapsed} />
            <LiveStats collapsed={sidebarCollapsed} />
            <UsagePanel collapsed={sidebarCollapsed} />
          </div>

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

        {/* Footer: a segmented toggle toolbar (keep-awake / theme) over
            the profile card. The toggles used to live inside the card, but at
            narrow sidebar widths they crowded out the app name — so they get
            their own bar. Mirrors the banner's `view_toggle` segmented control
            for visual consistency. The whole footer — toolbar and profile card
            alike — is dropped when the sidebar is collapsed: at 64px the card
            degenerates into a bare app icon plus a gear, which reads as clutter
            at the bottom of an icon rail. Settings stay reachable from the tray
            menu (contextMenu.ts) and from the new-session form. */}
        {!sidebarCollapsed && (
        <div className={styles.footer} data-wizard="settings-footer">
          <div className={styles.footer_toolbar} role="group" aria-label={t("settings.title")}>
            {keepAwakeSupported && (
              <button
                type="button"
                className={`${styles.footer_toolbar_btn} ${keepAwake ? styles.footer_toolbar_btn_active : ""}`}
                onClick={() => setKeepAwake(!keepAwake)}
                title={keepAwake ? t("keep_awake_on_tooltip") : t("keep_awake_off_tooltip")}
                aria-label={keepAwake ? t("keep_awake_on_tooltip") : t("keep_awake_off_tooltip")}
                aria-pressed={keepAwake}
              >
                <Coffee size={14} strokeWidth={1.5} />
              </button>
            )}
            <button
              type="button"
              className={`${styles.footer_toolbar_btn} ${styles.footer_theme_btn}`}
              onClick={() =>
                // Cycle light → dark → system → light. setTheme is global and
                // already re-skins the app + overlays, so no extra wiring.
                setTheme(theme === "light" ? "dark" : theme === "dark" ? "system" : "light")
              }
              title={t(`theme.${theme}`)}
              aria-label={t(`theme.${theme}`)}
            >
              {theme === "light" ? "☀" : theme === "dark" ? "☽" : "⊙"}
            </button>
          </div>
          {/* role=button, not <button>: keyboard a11y restored via
              tabIndex + onKeyDown. */}
          <div
            className={styles.footer_card}
            role="button"
            tabIndex={0}
            onClick={openSettings}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                openSettings();
              }
            }}
            title={t("settings.title")}
          >
            <div className={styles.footer_avatar}>
              <img src="/app-icon.png" alt="" style={{ width: "100%", height: "100%", objectFit: "contain" }} />
            </div>
            <div className={styles.footer_info}>
              <span className={styles.footer_name}>{t("title")}</span>
            </div>
            <span className={styles.footer_gear}>⚙</span>
          </div>
        </div>
        )}

        {/* Resize handle — hidden when collapsed */}
        {!sidebarCollapsed && (
          <ResizeHandle active={isDragging} onMouseDown={handleResizeMouseDown} />
        )}
      </aside>}

      {/* Main content area */}
      {viewMode === "history" ? (
        <HistoryView />
      ) : viewMode === "audit" ? (
        <AuditView />
      ) : viewMode === "memory" ? (
        <MemoryView />
      ) : viewMode === "schedule" ? (
        <ScheduleView />
      ) : viewMode === "plans" ? (
        <PlansView />
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
