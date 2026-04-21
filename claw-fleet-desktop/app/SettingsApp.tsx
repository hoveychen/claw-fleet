import { useCallback, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";
import { SettingsPanel } from "./components/SettingsPanel";
import { resolveTheme, useUIStore } from "./store";

function SettingsApp() {
  const { theme } = useUIStore();

  useEffect(() => {
    const apply = () => {
      const resolved = resolveTheme(theme);
      document.documentElement.setAttribute("data-theme", resolved);
      getCurrentWindow()
        .setTheme(resolved === "dark" ? "dark" : "light")
        .catch(() => {});
    };
    apply();

    if (theme === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      mq.addEventListener("change", apply);
      return () => mq.removeEventListener("change", apply);
    }
  }, [theme]);

  const handleClose = useCallback(() => {
    getCurrentWindow().close().catch(() => {});
  }, []);

  return <SettingsPanel onClose={handleClose} standalone />;
}

export default SettingsApp;
