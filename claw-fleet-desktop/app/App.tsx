import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
import "./fonts";
import "./App.css";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { LiteApp } from "./components/LiteApp";
import { Onboarding } from "./components/Onboarding";
import { SessionDetail } from "./components/SessionDetail";
import { SessionList } from "./components/SessionList";
import { WaitingAlerts } from "./components/WaitingAlerts";
import { DecisionPanel } from "./components/DecisionPanel";
import { UpdateNotice } from "./components/UpdateNotice";
import { Wizard } from "./components/Wizard";
import { WindowsFrameOverlay } from "./components/WindowsFrameOverlay";
import { useDecisionEvents } from "./hooks/useDecisionEvents";
import { useDecisionPeerSync } from "./hooks/useDecisionPeerSync";
import { useRuntimeTasksStore } from "./runtimeTasksStore";
import { type Connection, resolveTheme, useConnectionStore, useDecisionStore, useDetailStore, useSessionsStore, useUIStore } from "./store";
import { getItem, setItem, getSeenFeatures, ONBOARDING_FEATURES, type OnboardingFeatureId } from "./storage";
import type { OnboardingMode } from "./components/Onboarding";
import i18n from "./i18n";

const ONBOARDING_DISMISSED_KEY = "onboarding-dismissed";
const WIZARD_COMPLETED_KEY = "wizard-completed";

/** Compute which onboarding features the user hasn't seen yet. */
function computeUnseenFeatures(): OnboardingFeatureId[] {
  const seen = getSeenFeatures();
  return ONBOARDING_FEATURES.filter((id) => !seen.has(id));
}

function App() {
  const { theme, liteMode, setTheme, setLiteMode, setViewMode, setShowMobileAccess } = useUIStore();
  const { connection, setConnection, disconnect } = useConnectionStore();

  // Always-mounted listeners for backend decision events. Must live at the
  // App root so events aren't dropped while DecisionPanel is unmounted
  // (e.g. lite mode with no pending decisions).
  useDecisionEvents();
  useDecisionPeerSync();

  // Bridge: pop the floating decision window when the user can't see the in-app
  // DecisionPanel — either because the main window is minimized, or because the
  // user toggled "always use the standalone window" in Settings.
  const [mainMinimized, setMainMinimized] = useState(false);
  const decisions = useDecisionStore((s) => s.decisions);
  const floatingDecisionPanel = useUIStore((s) => s.floatingDecisionPanel);
  const prevShouldShow = useRef(false);

  useEffect(() => {
    const unlisten = listen<boolean>(
      "main-window-minimize-state-changed",
      (e) => setMainMinimized(!!e.payload),
    );
    invoke<boolean>("is_main_window_minimized")
      .then((v) => setMainMinimized(!!v))
      .catch(() => {});
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const shouldShow =
      (mainMinimized || floatingDecisionPanel) && decisions.length > 0;
    if (shouldShow && !prevShouldShow.current) {
      invoke("show_decision_float", { snapshot: decisions }).catch(() => {});
    } else if (!shouldShow && prevShouldShow.current) {
      invoke("hide_decision_float").catch(() => {});
    }
    prevShouldShow.current = shouldShow;
  }, [mainMinimized, floatingDecisionPanel, decisions]);

  const [onboardingMode, setOnboardingMode] = useState<OnboardingMode | null>(() => {
    const dismissed = !!getItem(ONBOARDING_DISMISSED_KEY);
    if (!dismissed) return "full";
    // Already dismissed — check for new features since last visit
    const unseen = computeUnseenFeatures();
    return unseen.length > 0 ? "whats_new" : null;
  });
  const [showWizard, setShowWizard] = useState(false);

  // Drag bar / caption buttons now switch off the host classes set in
  // main.tsx (`tauri-host` + `os-windows` / `os-macos`); we still need
  // backend confirmation of the OS to apply macOS-only window tweaks
  // (clearing the title, setting [data-platform="macos"] for legacy
  // module-CSS selectors).
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

  useEffect(() => {
    const unlisten = listen("switch-connection", () => {
      useDetailStore.getState().close();
      disconnect();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [disconnect]);

  // Sync theme/lang from other windows (standalone Settings, overlay).
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
    const unFloatingDecisionPromise = listen<boolean>(
      "overlay-floating-decision-panel-changed",
      (e) => {
        if (useUIStore.getState().floatingDecisionPanel !== e.payload) {
          useUIStore.setState({ floatingDecisionPanel: e.payload });
        }
      },
    );
    return () => {
      unThemePromise.then((fn) => fn());
      unLangPromise.then((fn) => fn());
      unMascotPromise.then((fn) => fn());
      unFloatingDecisionPromise.then((fn) => fn());
    };
  }, []);

  // Phase 3: hydrate + subscribe to the fleet-task runtime registry so the
  // app can show which tasks are currently backed by a live process.
  useEffect(() => {
    let unsubscribe: (() => void) | null = null;
    useRuntimeTasksStore.getState().refresh();
    useRuntimeTasksStore
      .getState()
      .subscribe()
      .then((unsub) => {
        unsubscribe = unsub;
      });
    return () => {
      unsubscribe?.();
    };
  }, []);

  // ── App-menu event handlers ────────────────────────────────────────
  // Forwarded by Rust's `on_menu_event` for items with `menu-*` ids.
  useEffect(() => {
    const ps: Promise<() => void>[] = [];

    ps.push(listen<"system" | "light" | "dark">("menu-theme", (e) => {
      setTheme(e.payload);
    }));
    ps.push(listen("menu-toggle-lite", () => {
      setLiteMode(!useUIStore.getState().liteMode);
    }));
    ps.push(listen("menu-daily-report", () => {
      setViewMode("report");
      if (useUIStore.getState().liteMode) setLiteMode(false);
    }));
    ps.push(listen("menu-welcome", () => {
      setOnboardingMode("full");
    }));
    ps.push(listen("menu-mobile-access", () => {
      setShowMobileAccess(true);
    }));
    ps.push(listen("menu-check-updates", async () => {
      try {
        const result = await invoke<{ has_update: boolean; latest_version: string; release_url: string }>(
          "check_app_version",
        );
        if (result.has_update && result.release_url) {
          const { openUrl } = await import("@tauri-apps/plugin-opener");
          await openUrl(result.release_url).catch(() => {});
        }
      } catch {
        /* network errors are silent */
      }
    }));

    return () => {
      ps.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, [setTheme, setLiteMode, setViewMode, setShowMobileAccess]);

  // Open a session detail when the user clicks an agent in the tray menu.
  useEffect(() => {
    const unlisten = listen<string>("open-session", (event) => {
      const jsonlPath = event.payload;
      const session = useSessionsStore.getState().sessions.find(
        (s) => s.jsonlPath === jsonlPath,
      );
      if (session) {
        useDetailStore.getState().open(session);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Sync initial UI locale to the Rust backend.
  useEffect(() => {
    invoke("set_locale", { locale: i18n.language }).catch(() => {});
  }, []);

  // Sync notification mode to Rust backend on startup (backend defaults to "user_action").
  useEffect(() => {
    const mode = getItem("notification-mode");
    if (mode) {
      invoke("set_notification_mode", { mode }).catch(() => {});
    }
  }, []);

  // Sync user title to Rust backend on startup.
  useEffect(() => {
    const title = getItem("user-title");
    if (title) {
      invoke("set_user_title", { title }).catch(() => {});
    }
  }, []);

  useEffect(() => {
    const apply = () => {
      const resolved = resolveTheme(theme);
      document.documentElement.setAttribute("data-theme", resolved);
      // setTheme triggers an NSAppearance change on macOS, which makes
      // AppKit relayout the standard window buttons back to the system
      // default — overriding our trafficLightPosition. Nudging the
      // content view forces tao to re-apply the inset on next draw.
      getCurrentWindow()
        .setTheme(resolved === "dark" ? "dark" : "light")
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

  const handleConnected = useCallback(
    (conn: Connection) => {
      setConnection(conn);
    },
    [setConnection]
  );

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

  // Re-apply window decorations/size when the saved liteMode differs from the
  // actual window state (e.g. first launch after a reload).
  useEffect(() => {
    invoke("set_lite_mode", { enabled: liteMode }).catch(() => {});
  }, [liteMode]);

  // Show connection dialog until the user picks local or remote
  if (!connection) {
    return (
      <div className="app">
        <WindowsFrameOverlay />
        <ConnectionDialog onConnected={handleConnected} />
      </div>
    );
  }

  if (liteMode) {
    return (
      <div className="app">
        <WindowsFrameOverlay />
        <LiteApp />
        <WaitingAlerts />
      </div>
    );
  }

  return (
    <div className="app">
      <WindowsFrameOverlay />
      {onboardingMode && <Onboarding mode={onboardingMode} onDismiss={finishOnboarding} />}
      {showWizard && <Wizard onDone={dismissWizard} />}
      <div className="app_main">
        <SessionList />
        <SessionDetail />
      </div>
      {!floatingDecisionPanel && <DecisionPanel />}
      <WaitingAlerts />
      <UpdateNotice />
    </div>
  );
}

export default App;
