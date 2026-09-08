import React from "react";
import ReactDOM from "react-dom/client";
import { stampHostClasses } from "./hostClass";
import { markWebBuild } from "./hostEnv";
import { primePromoStorage, promoSceneFromSearch } from "./mock/promo-scene";

const params = new URLSearchParams(window.location.search);
const isMockMode = params.has("mock") || import.meta.env.VITE_MOCK === "true";
const mockQaMode = params.has("qa");
const promoScene = promoSceneFromSearch(window.location.search);
// `?mock&demo` — the promo screencast board (real translated sessions + the
// 5-hop relay). Handled inside tauri-mock; here we just skip onboarding so the
// recording opens straight onto the populated board.
const demoMode = params.has("demo");

if (isMockMode && (promoScene || demoMode || params.has("website"))) {
  primePromoStorage(window.localStorage);
}
// The promo screencast is an English piece — pin the UI language so no chrome
// string (e.g. the composer placeholder) falls back to the boss's zh locale.
if (isMockMode && demoMode) {
  window.localStorage.setItem("mock-store:lang", "en");
}

if (isMockMode && params.has("website")) {
  markWebBuild();
  window.localStorage.setItem("mock-store:daily-report-last-popped", new Date().toISOString().slice(0, 10));
  const seen = JSON.parse(window.localStorage.getItem("mock-store:onboarding-seen-features") || "[]");
  window.localStorage.setItem("mock-store:onboarding-seen-features", JSON.stringify([...seen, "nav_modes"]));
  window.localStorage.setItem("mock-store:wizard-completed", "1");
  window.localStorage.setItem("mock-store:lang", params.get("website") === "zh" ? "zh" : "en");
}

stampHostClasses();

async function boot() {
  let triggerPromoScene: ((scene: NonNullable<typeof promoScene>) => void) | null = null;
  let triggerMockQaScenario: (() => void) | null = null;
  // Whether this is the desktop webview. Must be read *before* the web
  // transport installs, because that installs `__TAURI_INTERNALS__` itself.
  let isTauriBuild = true;
  // In mock mode, install the Tauri API fakes BEFORE anything else loads.
  if (isMockMode) {
    const mocks = await import("./mock/tauri-mock");
    const { installMocks } = mocks;
    installMocks({ qaMode: mockQaMode });
    triggerPromoScene = mocks.triggerPromoScene;
    triggerMockQaScenario = mocks.triggerMockQaScenario;
  } else {
    // Same bundle, opened in a plain browser rather than the desktop webview:
    // stand an HTTP transport in for Tauri's IPC. Must also precede everything
    // else — `initStorage()` below is already an `invoke` call.
    const { isTauriHost, installWebTransport } = await import("./webTransport");
    isTauriBuild = isTauriHost();
    if (!isTauriBuild) {
      // Before the transport, which installs `__TAURI_INTERNALS__` itself and
      // so destroys the evidence `isTauriHost()` just read.
      markWebBuild();
      await installWebTransport();
      // Cache the hash-named /assets/ chunks so a redeploy only re-downloads
      // what actually changed — the Office preview alone is ~1.6 MB of lazily
      // loaded code. Browser build only: the desktop webview loads the same
      // files off disk, and `?mock` must keep reading as the desktop. Not
      // awaited — registration is not on the critical path to first paint.
      if ("serviceWorker" in navigator) {
        navigator.serviceWorker.register(`${import.meta.env.BASE_URL}sw.js`).catch(() => {
          // No SW (insecure origin, private window, disabled) just means every
          // asset comes off the network. Nothing else depends on it.
        });
      }
    }
  }

  // Time every invoke from here on. After the transport/mock branch above,
  // because both of those install `__TAURI_INTERNALS__` wholesale and would
  // drop the wrapper; before `initStorage()`, which is itself an invoke.
  // Desktop only: the log it writes to is a host file, and the round trip it
  // watches for is the desktop event loop's.
  if (isTauriBuild) {
    const { installInvokeProbe } = await import("./invokeProbe");
    installInvokeProbe();
  }

  const { initStorage, migrateSessionViewDefault, migrateFeatureTristate } =
    await import("./storage");

  // Load persisted settings into memory before anything reads them.
  await initStorage();

  // Roll out gallery as the default session view for existing users whose disk
  // still carries a stale "list". Must run before the UIStore is constructed
  // (i.e. before ./App is imported below).
  migrateSessionViewDefault();

  // Reset the changed-default feature keys to the "default" (unset) state once,
  // so existing users follow the new central defaults instead of a stale binary
  // value left by the old mount reconciliation. Must run before any feature
  // read (SettingsPanel/Onboarding state inits, hook auto-apply).
  migrateFeatureTristate();

  // i18n must be imported after storage is ready (it reads "lang" synchronously).
  await import("./i18n");

  const { installAppContextMenu } = await import("./contextMenu");
  installAppContextMenu();

  const { default: App } = await import("./App");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );

  if (promoScene && triggerPromoScene) {
    window.setTimeout(() => triggerPromoScene?.(promoScene), 900);
  }
  if (mockQaMode && triggerMockQaScenario) {
    window.setTimeout(() => triggerMockQaScenario?.(), 900);
  }
}

boot();
