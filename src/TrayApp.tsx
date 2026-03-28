import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import "./App.css";
import { resolveTheme, useSessionsStore, useUIStore } from "./store";
import type { SessionInfo } from "./types";
import { TrayPanel } from "./components/TrayPanel";

interface UsageBarData {
  label: string;
  utilization: number;
}

interface UsageSummary {
  source: string;
  bars: UsageBarData[];
}

function TrayApp() {
  const { theme } = useUIStore();
  const [usageSummaries, setUsageSummaries] = useState<UsageSummary[]>([]);

  // Apply theme — keep body transparent for vibrancy to show through
  useEffect(() => {
    const apply = () => {
      document.documentElement.setAttribute("data-theme", resolveTheme(theme));
      document.body.style.background = "transparent";
      document.documentElement.style.background = "transparent";
    };
    apply();
    if (theme === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      mq.addEventListener("change", apply);
      return () => mq.removeEventListener("change", apply);
    }
  }, [theme]);

  // Listen for theme changes from main window
  useEffect(() => {
    const unlisten = listen<string>("overlay-theme-changed", (e) => {
      useUIStore.setState({ theme: e.payload as "dark" | "light" | "system" });
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // Listen for language changes from main window
  const { i18n } = useTranslation();
  useEffect(() => {
    const unlisten = listen<string>("overlay-lang-changed", (e) => {
      i18n.changeLanguage(e.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, [i18n]);

  // Poll sessions
  useEffect(() => {
    const refresh = async () => {
      try {
        const sessions = await invoke<SessionInfo[]>("list_sessions");
        useSessionsStore.getState().setSessions(sessions);
      } catch { /* ignore */ }
    };
    refresh();
    const id = setInterval(refresh, 3000);
    return () => clearInterval(id);
  }, []);

  // Listen for usage summaries from backend
  useEffect(() => {
    const unlisten = listen<UsageSummary[]>("tray-usage-updated", (e) => {
      setUsageSummaries(e.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // Auto-hide on blur is handled entirely in Rust (on_window_event)
  // to avoid IPC timing issues with macOS spurious blur events.

  return <TrayPanel usageSummaries={usageSummaries} />;
}

export default TrayApp;
