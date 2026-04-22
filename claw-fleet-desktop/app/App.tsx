import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
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
import { useDecisionEvents } from "./hooks/useDecisionEvents";
import { useDecisionPeerSync } from "./hooks/useDecisionPeerSync";
import { type Connection, resolveTheme, useConnectionStore, useDecisionStore, useDetailStore, useOverlayStore, useSessionsStore, useUIStore } from "./store";
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

  // Bridge: when the main window is minimized and there are pending decisions,
  // pop a floating decision window on the cursor's monitor bottom-center.
  const [mainMinimized, setMainMinimized] = useState(false);
  const decisions = useDecisionStore((s) => s.decisions);
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
    const shouldShow = mainMinimized && decisions.length > 0;
    if (shouldShow && !prevShouldShow.current) {
      invoke("show_decision_float", { snapshot: decisions }).catch(() => {});
    } else if (!shouldShow && prevShouldShow.current) {
      invoke("hide_decision_float").catch(() => {});
    }
    prevShouldShow.current = shouldShow;
  }, [mainMinimized, decisions]);

  const [isMacOS, setIsMacOS] = useState(false);
  const [onboardingMode, setOnboardingMode] = useState<OnboardingMode | null>(() => {
    const dismissed = !!getItem(ONBOARDING_DISMISSED_KEY);
    if (!dismissed) return "full";
    // Already dismissed — check for new features since last visit
    const unseen = computeUnseenFeatures();
    return unseen.length > 0 ? "whats_new" : null;
  });
  const [showWizard, setShowWizard] = useState(false);

  useEffect(() => {
    invoke<string>("get_platform").then((p) => {
      if (p === "macos") {
        setIsMacOS(true);
        getCurrentWindow().setTitle("").catch(() => {});
        document.documentElement.setAttribute("data-platform", "macos");
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
    return () => {
      unThemePromise.then((fn) => fn());
      unLangPromise.then((fn) => fn());
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

  // Sync overlay store when overlay is closed from the overlay window (right-click)
  useEffect(() => {
    const unlisten = listen("overlay-disabled", () => {
      // Update store + storage without re-invoking toggle_overlay (already done by overlay)
      setItem("overlay-enabled", "false");
      useOverlayStore.setState({ enabled: false });
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // Restore overlay on app startup if it was previously enabled
  useEffect(() => {
    if (useOverlayStore.getState().enabled) {
      invoke("toggle_overlay", { visible: true }).catch(() => {});
    }
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
      getCurrentWindow().setTheme(resolved === "dark" ? "dark" : "light").catch(() => {});
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
        {isMacOS && !liteMode && <div className="drag_bar" data-tauri-drag-region />}
        <ConnectionDialog onConnected={handleConnected} />
      </div>
    );
  }

  if (liteMode) {
    return (
      <div className="app">
        <LiteApp />
        <WaitingAlerts />
      </div>
    );
  }

  return (
    <div className="app">
      {isMacOS && <div className="drag_bar" data-tauri-drag-region />}
      {onboardingMode && <Onboarding mode={onboardingMode} onDismiss={finishOnboarding} />}
      {showWizard && <Wizard onDone={dismissWizard} />}
      <div className="app_main">
        <SessionList />
        <SessionDetail />
      </div>
      <DecisionPanel />
      <WaitingAlerts />
      <UpdateNotice />
    </div>
  );
}

export default App;
