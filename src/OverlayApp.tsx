import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import "./App.css";
import { getItem } from "./storage";
import { resolveTheme, useSessionsStore, useUIStore, useWaitingAlertsStore } from "./store";
import type { SessionInfo, WaitingAlert } from "./types";

/** Lazy-load OverlayMascot to isolate render errors */
let OverlayMascot: React.ComponentType | null = null;

function OverlayApp() {
  const { theme } = useUIStore();
  const [ready, setReady] = useState(false);
  const [loadError, setLoadError] = useState("");

  // Auto-show if overlay was previously enabled — use the Rust command
  // which handles positioning on-screen (avoids transparent window bugs).
  useEffect(() => {
    if (getItem("overlay-enabled") === "true") {
      invoke("toggle_overlay", { visible: true }).catch(() => {});
    }
  }, []);

  // Apply theme + force transparent body for overlay window
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

  // Listen for theme changes from main window.
  // Use setState directly to avoid re-emitting overlay-theme-changed (which would create a loop).
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

  // Listen for waiting alerts (audio is played by WaitingAlerts in the main window)
  useEffect(() => {
    const { refresh, setAlerts } = useWaitingAlertsStore.getState();
    refresh().catch((e) => console.warn("[overlay] initial alert refresh failed:", e));
    const unlisten = listen<WaitingAlert[]>("waiting-alerts-updated", (e) => {
      console.debug("[overlay] waiting-alerts-updated:", e.payload.length, "alerts");
      setAlerts(e.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // Lazy-load OverlayMascot
  useEffect(() => {
    import("./components/OverlayMascot")
      .then((mod) => {
        OverlayMascot = mod.OverlayMascot;
        setReady(true);
      })
      .catch((e) => {
        setLoadError(String(e));
      });
  }, []);

  if (loadError) {
    return (
      <div style={{ color: "red", padding: 12, fontSize: 11, background: "rgba(0,0,0,0.85)", borderRadius: 12, margin: 8 }}>
        Overlay load error: {loadError}
      </div>
    );
  }

  if (!ready || !OverlayMascot) {
    // Minimal visible placeholder so we can tell the window is alive
    return (
      <div style={{
        width: "100%",
        height: "100%",
        display: "flex",
        alignItems: "flex-end",
        justifyContent: "center",
      }}>
        <div style={{
          width: 260,
          height: 80,
          background: "rgba(20, 20, 28, 0.92)",
          borderRadius: 20,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: "#5a5a78",
          fontSize: 11,
          border: "1px solid rgba(255,255,255,0.08)",
        }}>
          Loading...
        </div>
      </div>
    );
  }

  return <OverlayMascot />;
}

export default OverlayApp;
