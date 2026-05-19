import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./fonts";
import "./App.css";
import { DecisionPanel } from "./components/DecisionPanel";
import { useDecisionEvents } from "./hooks/useDecisionEvents";
import { useDecisionPeerSync } from "./hooks/useDecisionPeerSync";
import { resolveTheme, useDecisionStore, useSessionsStore, useUIStore } from "./store";
import i18n from "./i18n";
import type { PendingDecision, SessionInfo } from "./types";

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

  // Mirror the main window's sessions state so SessionDetail (and any other
  // session-aware UI inside DecisionPanel) renders correctly here. Backend
  // commands and `sessions-updated` events are broadcast to every window, so
  // we just need to subscribe + seed once.
  useEffect(() => {
    useSessionsStore.getState().refresh().catch(() => {});
    const unlistenSessions = listen<SessionInfo[]>("sessions-updated", (e) => {
      useSessionsStore.getState().setSessions(e.payload);
    });
    const unlistenScan = listen<boolean>("scan-ready", () => {
      useSessionsStore.getState().setScanReady(true);
    });
    return () => {
      unlistenSessions.then((fn) => fn());
      unlistenScan.then((fn) => fn());
    };
  }, []);

  // Platform attribute so macOS-specific CSS in App.css applies (matches main App).
  useEffect(() => {
    invoke<string>("get_platform")
      .then((p) => {
        if (p === "macos") {
          document.documentElement.setAttribute("data-platform", "macos");
        }
      })
      .catch(() => {});
  }, []);

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

  // Window width follows DecisionPanel's inline SessionDetail state: narrow
  // when only the decision card is visible, wide enough for the side-by-side
  // detail column when the user expands history.
  const [inlineDetailOpen, setInlineDetailOpen] = useState(false);
  const targetWidth = inlineDetailOpen ? 920 : 480;
  const targetWidthRef = useRef(targetWidth);
  useEffect(() => {
    targetWidthRef.current = targetWidth;
    invoke("resize_decision_float", { width: targetWidth }).catch(() => {});
  }, [targetWidth]);

  // Content-driven height: observe the inner wrapper and push its natural
  // height to Rust so the float window auto-fits the DecisionPanel.
  const innerRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!innerRef.current) return;
    const el = innerRef.current;
    let timer: number | null = null;
    let lastH = 0;
    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const h = Math.ceil(entry.contentRect.height);
      if (Math.abs(h - lastH) < 4) return;
      lastH = h;
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        invoke("resize_decision_float", {
          width: targetWidthRef.current,
          height: h,
        }).catch(() => {});
      }, 100);
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (timer !== null) window.clearTimeout(timer);
    };
  }, []);

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: "var(--color-bg-sidebar, #141414)",
        overflowX: "hidden",
        overflowY: "auto",
      }}
    >
      <div ref={innerRef}>
        <DecisionPanel onInlineDetailChange={setInlineDetailOpen} />
      </div>
    </div>
  );
}
