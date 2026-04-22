import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DecisionPanel } from "./components/DecisionPanel";
import { useDecisionEvents } from "./hooks/useDecisionEvents";
import { useDecisionPeerSync } from "./hooks/useDecisionPeerSync";
import { resolveTheme, useDecisionStore, useUIStore } from "./store";
import i18n from "./i18n";
import type { PendingDecision } from "./types";

export default function DecisionFloatApp() {
  // Subscribe to backend decision events so decisions arriving while the float
  // is open are captured live. Silent because the main window already chimes.
  useDecisionEvents({ silent: true });

  // Cross-window dismissal sync: if the main window responds to a decision
  // while this float is open, drop it here too (and vice versa).
  useDecisionPeerSync();

  // On mount, hydrate from the snapshot the main window seeded in AppState.
  useEffect(() => {
    (async () => {
      try {
        const snap = await invoke<PendingDecision[] | null>(
          "get_decision_float_snapshot",
        );
        if (snap && snap.length > 0) {
          useDecisionStore.setState({
            decisions: snap,
            activeDecisionId: snap[0].id,
          });
        }
      } catch (e) {
        console.warn("[decision-float] snapshot fetch failed:", e);
      }
    })();
  }, []);

  // Auto-hide once the queue drains (user answered every decision).
  const decisionCount = useDecisionStore((s) => s.decisions.length);
  useEffect(() => {
    if (decisionCount === 0) {
      const t = setTimeout(() => {
        invoke("hide_decision_float").catch(() => {});
      }, 180);
      return () => clearTimeout(t);
    }
  }, [decisionCount]);

  // Theme + language sync across windows.
  useEffect(() => {
    resolveTheme(useUIStore.getState().theme);
    const unThemePromise = listen<string>("overlay-theme-changed", (e) => {
      const next = e.payload as "dark" | "light" | "system";
      useUIStore.setState({ theme: next });
      resolveTheme(next);
    });
    const unLangPromise = listen<string>("overlay-lang-changed", (e) => {
      if (i18n.language !== e.payload) i18n.changeLanguage(e.payload);
    });
    return () => {
      unThemePromise.then((fn) => fn());
      unLangPromise.then((fn) => fn());
    };
  }, []);

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: "var(--color-bg-sidebar, #141414)",
        overflow: "hidden",
      }}
    >
      <DecisionPanel />
    </div>
  );
}
