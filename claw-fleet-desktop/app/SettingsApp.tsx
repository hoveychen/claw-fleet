import { useCallback, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./fonts";
import "./App.css";
import { SettingsPanel } from "./components/SettingsPanel";
import { applyWindowTheme, useUIStore } from "./store";

function SettingsApp() {
  const { theme } = useUIStore();

  useEffect(() => {
    const apply = () => {
      applyWindowTheme(theme).catch(() => {});
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
