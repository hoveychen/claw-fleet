import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useState } from "react";
import "./App.css";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { Onboarding } from "./components/Onboarding";
import { SessionDetail } from "./components/SessionDetail";
import { SessionList } from "./components/SessionList";
import { WaitingAlerts } from "./components/WaitingAlerts";
import { UpdateNotice } from "./components/UpdateNotice";
import { Wizard } from "./components/Wizard";
import { type Connection, resolveTheme, useConnectionStore, useDetailStore, useOverlayStore, useSessionsStore, useUIStore } from "./store";
import { getItem, setItem } from "./storage";
import i18n from "./i18n";

const ONBOARDING_DISMISSED_KEY = "onboarding-dismissed";
const WIZARD_COMPLETED_KEY = "wizard-completed";

function App() {
  const { theme } = useUIStore();
  const { connection, setConnection, disconnect } = useConnectionStore();

  const [isMacOS, setIsMacOS] = useState(false);
  const [showOnboarding, setShowOnboarding] = useState(() => {
    return !getItem(ONBOARDING_DISMISSED_KEY);
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
    setShowOnboarding(false);
    setItem(ONBOARDING_DISMISSED_KEY, "1");
    if (!getItem(WIZARD_COMPLETED_KEY)) {
      setShowWizard(true);
    }
  }, []);

  const dismissWizard = useCallback(() => {
    setShowWizard(false);
    setItem(WIZARD_COMPLETED_KEY, "1");
  }, []);

  // Show connection dialog until the user picks local or remote
  if (!connection) {
    return (
      <div className="app">
        {isMacOS && <div className="drag_bar" data-tauri-drag-region />}
        <ConnectionDialog onConnected={handleConnected} />
      </div>
    );
  }

  return (
    <div className="app">
      {isMacOS && <div className="drag_bar" data-tauri-drag-region />}
      {showOnboarding && <Onboarding onDismiss={finishOnboarding} />}
      {showWizard && <Wizard onDone={dismissWizard} />}
      <SessionList />
      <SessionDetail />
      <WaitingAlerts />
      <UpdateNotice />
    </div>
  );
}

export default App;
