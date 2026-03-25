import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import "./App.css";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { Onboarding } from "./components/Onboarding";
import { SessionDetail } from "./components/SessionDetail";
import { SessionList } from "./components/SessionList";
import { WaitingAlerts } from "./components/WaitingAlerts";
import { Wizard } from "./components/Wizard";
import { type Connection, resolveTheme, useConnectionStore, useDetailStore, useUIStore } from "./store";
import { getItem, setItem } from "./storage";
import i18n from "./i18n";

const ONBOARDING_DISMISSED_KEY = "onboarding-dismissed";
const WIZARD_COMPLETED_KEY = "wizard-completed";

function App() {
  const { theme } = useUIStore();
  const { connection, setConnection, disconnect } = useConnectionStore();

  const [showOnboarding, setShowOnboarding] = useState(() => {
    return !getItem(ONBOARDING_DISMISSED_KEY);
  });
  const [showWizard, setShowWizard] = useState(false);

  useEffect(() => {
    const unlisten = listen("switch-connection", () => {
      useDetailStore.getState().close();
      disconnect();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [disconnect]);

  // Sync initial UI locale to the Rust backend.
  useEffect(() => {
    invoke("set_locale", { locale: i18n.language }).catch(() => {});
  }, []);

  useEffect(() => {
    const apply = () => {
      document.documentElement.setAttribute("data-theme", resolveTheme(theme));
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
        <ConnectionDialog onConnected={handleConnected} />
      </div>
    );
  }

  return (
    <div className="app">
      {showOnboarding && <Onboarding onDismiss={finishOnboarding} />}
      {showWizard && <Wizard onDone={dismissWizard} />}
      <SessionList />
      <SessionDetail />
      <WaitingAlerts />
    </div>
  );
}

export default App;
