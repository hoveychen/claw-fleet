import { useCallback, useEffect, useState } from "react";
import "./App.css";
import { ConnectionDialog } from "./components/ConnectionDialog";
import type { RemoteConnection } from "./components/ConnectionDialog";
import { Onboarding } from "./components/Onboarding";
import { SessionDetail } from "./components/SessionDetail";
import { SessionList } from "./components/SessionList";
import { Wizard } from "./components/Wizard";
import { resolveTheme, useConnectionStore, useUIStore } from "./store";

const ONBOARDING_DISMISSED_KEY = "onboarding-dismissed";
const WIZARD_COMPLETED_KEY = "wizard-completed";

function App() {
  const { theme, viewMode } = useUIStore();
  const { connected, setConnected } = useConnectionStore();

  const [showOnboarding, setShowOnboarding] = useState(() => {
    return !localStorage.getItem(ONBOARDING_DISMISSED_KEY);
  });
  const [showWizard, setShowWizard] = useState(false);

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
    (remote: RemoteConnection | null) => {
      setConnected(remote);
    },
    [setConnected]
  );

  const finishOnboarding = useCallback(() => {
    setShowOnboarding(false);
    localStorage.setItem(ONBOARDING_DISMISSED_KEY, "1");
    if (!localStorage.getItem(WIZARD_COMPLETED_KEY)) {
      setShowWizard(true);
    }
  }, []);

  const dismissWizard = useCallback(() => {
    setShowWizard(false);
    localStorage.setItem(WIZARD_COMPLETED_KEY, "1");
  }, []);

  // Show connection dialog until the user picks local or remote
  if (!connected) {
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
      {viewMode === "list" && <SessionDetail />}
    </div>
  );
}

export default App;
