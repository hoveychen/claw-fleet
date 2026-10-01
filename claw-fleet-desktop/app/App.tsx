import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
import "./fonts";
import "./App.css";
import { Onboarding } from "./components/Onboarding";
import { SessionList } from "./components/SessionList";
import { SettingsPanel } from "./components/SettingsPanel";
import { DecisionPanel } from "./components/DecisionPanel";
import { DailyReportPopup } from "./components/report/DailyReportPopup";
import { FindBar } from "./components/FindBar";
import { useFindController } from "./find/useFindController";
import { UpdateNotice } from "./components/UpdateNotice";
import { versionCheckArgs } from "./versionCheck";
import { Wizard } from "./components/Wizard";
import { WindowsFrameOverlay } from "./components/WindowsFrameOverlay";
import { useDecisionEvents } from "./hooks/useDecisionEvents";
import { applyWindowTheme, navigateToSessionDetail, useReportStore, useSessionsStore, useUIStore } from "./store";
import { getItem, setItem, getSeenFeatures, ONBOARDING_FEATURES, type OnboardingFeatureId } from "./storage";
import type { OnboardingMode } from "./components/Onboarding";
import i18n from "./i18n";
import { localDateKeyDaysAgo } from "./localDate";
import { useRemoteWorkspacesSync } from "./hooks/useRemoteWorkspaces";
import { useWaitingAlertSound } from "./hooks/useWaitingAlertSound";
import { Presence } from "./components/Presence";
import { Spinner } from "./components/loading";
import styles from "./App.module.css";

const ONBOARDING_DISMISSED_KEY = "onboarding-dismissed";
const WIZARD_COMPLETED_KEY = "wizard-completed";

/** Compute which onboarding features the user hasn't seen yet. */
function computeUnseenFeatures(): OnboardingFeatureId[] {
  const seen = getSeenFeatures();
  return ONBOARDING_FEATURES.filter((id) => !seen.has(id));
}

function App() {
  const { theme, setTheme, setViewMode } = useUIStore();

  // Always-mounted listeners for backend decision events. Must live at the
  // App root so events aren't dropped while DecisionPanel is unmounted
  // (e.g. no pending decisions).
  useDecisionEvents();

  // The rca registry, fetched once for the whole app: the session card, list
  // and tab strip all badge remote workspaces from it, and a per-card fetch
  // would be one IPC round trip per card per board render.
  useRemoteWorkspacesSync();

  // Chime/TTS when a session starts waiting for input. Headless — the
  // bottom-right alert cards this used to live in were dropped; only the
  // sound survives.
  useWaitingAlertSound();

  // Settings overlay. Lives in the store rather than component state because
  // the tray/app menu (a Rust-side event) and the sidebar gear button are both
  // entry points, and the panel used to be a separate window every caller
  // reached through an `invoke`.
  const settingsOpen = useUIStore((s) => s.settingsOpen);
  const setSettingsOpen = useUIStore((s) => s.setSettingsOpen);
  const closeSettings = useCallback(() => setSettingsOpen(false), [setSettingsOpen]);

  // In-app Cmd/Ctrl+F find bar. The controller's key listener is global, so the
  // bar can be summoned from any view; we render it in the searchable returns.
  const find = useFindController();

  const simplifiedMode = useUIStore((s) => s.simplifiedMode);

  const [onboardingMode, setOnboardingMode] = useState<OnboardingMode | null>(() => {
    const dismissed = !!getItem(ONBOARDING_DISMISSED_KEY);
    if (!dismissed) return "full";
    // Already dismissed — check for new features since last visit
    const unseen = computeUnseenFeatures();
    return unseen.length > 0 ? "whats_new" : null;
  });
  const [showWizard, setShowWizard] = useState(false);
  // On-screen feedback for the native menu's "Check for updates": the menu item
  // itself cannot show progress, so a small pill reports checking / result.
  const [updateCheck, setUpdateCheck] = useState<"checking" | "latest" | "failed" | null>(null);
  const updateCheckRef = useRef(false);

  // Drag bar / caption buttons now switch off the host classes set in
  // main.tsx (`tauri-host` + `os-windows` / `os-macos`); we still need
  // backend confirmation of the OS to apply macOS-only window tweaks
  // (clearing the title, setting [data-platform="macos"] for legacy
  // module-CSS selectors).
  // Which optional surfaces this backend allows (currently: the 「终端」(Terminal)
  // page, gated by FLEET_TERMINAL). Read once — it is a launch property of the
  // backend process, so polling it would only ever get the same answer.
  useEffect(() => {
    void useUIStore.getState().loadHostFeatures();
  }, []);

  useEffect(() => {
    invoke<string>("get_platform").then((p) => {
      if (p === "macos") {
        getCurrentWindow().setTitle("").catch(() => {});
        document.documentElement.setAttribute("data-platform", "macos");
      } else if (p === "windows") {
        // Windows gets the same Liquid Glass sidebar skin as macOS — the
        // module-CSS rules key off [data-platform], so flag the platform
        // here. No setTitle: native chrome is already stripped via
        // set_decorations(false) in gui.rs.
        document.documentElement.setAttribute("data-platform", "windows");
      }
    });
  }, []);

  // Sync theme/lang from the tray/overlay mascot process.
  useEffect(() => {
    const unThemePromise = listen<string>("overlay-theme-changed", (e) => {
      const next = e.payload as "dark" | "light" | "system";
      if (useUIStore.getState().theme !== next) {
        useUIStore.setState({ theme: next });
      }
    });
    const unLangPromise = listen<string>("overlay-lang-changed", (e) => {
      if (i18n.language !== e.payload) {
        i18n.changeLanguage(e.payload);
      }
    });
    const unMascotPromise = listen<boolean>("overlay-mascot-visible-changed", (e) => {
      if (useUIStore.getState().mascotVisible !== e.payload) {
        useUIStore.setState({ mascotVisible: e.payload });
      }
    });
    return () => {
      unThemePromise.then((fn) => fn());
      unLangPromise.then((fn) => fn());
      unMascotPromise.then((fn) => fn());
    };
  }, []);

  // Catch-up popup. The `daily-report-ready` event only reaches a running app,
  // and the summary for a given day is usually written while the app is closed
  // (or during the 10s the scheduler waits before its first pass). So on boot
  // we ask directly whether yesterday's report is finished; `maybePopupReport`
  // is idempotent per date, so this never double-fires with the event.
  useEffect(() => {
    const d = localDateKeyDaysAgo(1);
    const t = window.setTimeout(() => {
      void useReportStore.getState().maybePopupReport(d);
    }, 1500);
    return () => window.clearTimeout(t);
  }, []);

  // ── App-menu event handlers ────────────────────────────────────────
  // Forwarded by Rust's `on_menu_event` for items with `menu-*` ids.
  useEffect(() => {
    const ps: Promise<() => void>[] = [];

    ps.push(listen<"system" | "light" | "dark">("menu-theme", (e) => {
      setTheme(e.payload);
    }));
    // The native app / tray menu's Settings item. Rust shows the main window
    // first, then emits this — there is no second window to build any more.
    ps.push(listen("menu-settings", () => {
      setSettingsOpen(true);
    }));
    ps.push(listen("menu-daily-report", () => {
      setViewMode("report");
    }));
    // The report scheduler announces a date the moment its AI summary lands.
    // Rust has already raised the main window by the time this arrives.
    ps.push(listen<string>("daily-report-ready", (e) => {
      void useReportStore.getState().maybePopupReport(e.payload);
    }));
    ps.push(listen("menu-welcome", () => {
      setOnboardingMode("full");
    }));
    ps.push(listen("menu-check-updates", async () => {
      // A second click while a check is in flight is dropped.
      if (updateCheckRef.current) return;
      updateCheckRef.current = true;
      setUpdateCheck("checking");
      let outcome: "latest" | "failed" | null = null;
      try {
        const result = await invoke<{ has_update: boolean; latest_version: string; release_url: string }>(
          "check_app_version",
          versionCheckArgs(true, i18n.resolvedLanguage ?? i18n.language),
        );
        if (result.has_update && result.release_url) {
          const { openUrl } = await import("@tauri-apps/plugin-opener");
          await openUrl(result.release_url).catch(() => {});
        } else {
          outcome = "latest";
        }
      } catch {
        outcome = "failed";
      } finally {
        updateCheckRef.current = false;
      }
      setUpdateCheck(outcome);
      if (outcome) {
        window.setTimeout(() => {
          setUpdateCheck((cur) => (cur === outcome ? null : cur));
        }, 2500);
      }
    }));

    return () => {
      ps.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, [setTheme, setViewMode, setSettingsOpen]);

  // Open a session detail when the user clicks an agent in the tray menu: the
  // 「任务」(Tasks) page's inline detail (see navigateToSessionDetail).
  useEffect(() => {
    const unlisten = listen<string>("open-session", (event) => {
      const jsonlPath = event.payload;
      const session = useSessionsStore.getState().sessions.find(
        (s) => s.jsonlPath === jsonlPath,
      );
      if (session) {
        navigateToSessionDetail(session);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Push locale + user title to the Rust backend, which re-renders the launch
  // guidance Fleet hands its own sessions with them. Nothing is installed into
  // ~/.claude any more, so there is no control plane to self-heal here.
  useEffect(() => {
    const title = getItem("user-title");
    invoke("set_locale", { locale: i18n.language }).catch(() => {});
    if (title) invoke("set_user_title", { title }).catch(() => {});
  }, []);

  // Sync notification mode to Rust backend on startup (backend defaults to "user_action").
  useEffect(() => {
    const mode = getItem("notification-mode");
    if (mode) {
      invoke("set_notification_mode", { mode }).catch(() => {});
    }
  }, []);

  useEffect(() => {
    const apply = () => {
      // setTheme triggers an NSAppearance change on macOS, which makes
      // AppKit relayout the standard window buttons back to the system
      // default — overriding our trafficLightPosition. Nudging the
      // content view forces tao to re-apply the inset on next draw.
      applyWindowTheme(theme)
        .then(() => invoke("nudge_traffic_lights"))
        .catch(() => {});
    };
    apply();

    if (theme === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      mq.addEventListener("change", apply);
      return () => mq.removeEventListener("change", apply);
    }
  }, [theme]);

  const finishOnboarding = useCallback(() => {
    setOnboardingMode(null);
    setItem(ONBOARDING_DISMISSED_KEY, "1");
    if (!getItem(WIZARD_COMPLETED_KEY)) {
      setShowWizard(true);
    }
  }, []);

  const dismissWizard = useCallback(() => {
    setShowWizard(false);
    setItem(WIZARD_COMPLETED_KEY, "1");
  }, []);

  return (
    <div className={simplifiedMode ? "app app_simplified" : "app"}>
      <WindowsFrameOverlay />
      {onboardingMode && <Onboarding mode={onboardingMode} onDismiss={finishOnboarding} />}
      {showWizard && <Wizard onDone={dismissWizard} />}
      {/* data-find-content scopes the Cmd+F find bar to the active page's
          content; the sidebar nav lives inside here too but is skipped by tag
          (<aside>/<nav>/<button>), and everything outside app_main (onboarding,
          decision panel) is excluded by not being tagged. */}
      <div className="app_main" data-find-content>
        <SessionList />
      </div>
      {!simplifiedMode && <DecisionPanel />}
      <Presence when={settingsOpen}>{settingsOpen && <SettingsPanel onClose={closeSettings} />}</Presence>
      {!simplifiedMode && <DailyReportPopup />}
      <UpdateNotice />
      <Presence when={!!updateCheck}>
        {updateCheck && (
          <div className={styles.menu_status} role="status">
            {updateCheck === "checking" && <Spinner size={12} />}
            {updateCheck === "checking"
              ? i18n.t("update.checking", "正在检查更新…")
              : updateCheck === "latest"
                ? i18n.t("update.up_to_date", "已是最新版本")
                : i18n.t("update.check_failed", "检查更新失败，请检查网络")}
          </div>
        )}
      </Presence>
      <FindBar controller={find} />
    </div>
  );
}

export default App;
