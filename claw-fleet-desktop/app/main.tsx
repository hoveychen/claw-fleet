import React from "react";
import ReactDOM from "react-dom/client";
import { stampHostClasses } from "./hostClass";
import { markWebBuild } from "./hostEnv";
import { localDateKey } from "./localDate";
import { primePromoStorage, promoSceneFromSearch } from "./mock/promo-scene";

const params = new URLSearchParams(window.location.search);
const isMockMode = params.has("mock") || import.meta.env.VITE_MOCK === "true";
const mockQaMode = params.has("qa");
const promoScene = promoSceneFromSearch(window.location.search);
// `?mock&demo` — the promo screencast board (real translated sessions + the
// 5-hop relay). Handled inside tauri-mock; here we just skip onboarding so the
// recording opens straight onto the populated board.
const demoMode = params.has("demo");

// `?mock&qa` fires a decision card 900ms after boot (see `triggerMockQaScenario`),
// which is the only way to eyeball the DecisionPanel without waiting for a real
// one — but the welcome page sat on top of it, so the card was unreachable
// unless you hand-primed localStorage first. Skip onboarding here too.
if (isMockMode && (promoScene || demoMode || mockQaMode || params.has("website"))) {
  primePromoStorage(window.localStorage);
}
// The promo screencast is an English piece — pin the UI language so no chrome
// string (e.g. the composer placeholder) falls back to the boss's zh locale.
if (isMockMode && demoMode) {
  window.localStorage.setItem("mock-store:lang", "en");
}

if (isMockMode && params.has("website")) {
  markWebBuild();
  window.localStorage.setItem("mock-store:daily-report-last-popped", localDateKey());
  const seen = JSON.parse(window.localStorage.getItem("mock-store:onboarding-seen-features") || "[]");
  window.localStorage.setItem("mock-store:onboarding-seen-features", JSON.stringify([...seen, "nav_more"]));
  window.localStorage.setItem("mock-store:wizard-completed", "1");
  window.localStorage.setItem("mock-store:lang", params.get("website") === "zh" ? "zh" : "en");
}

stampHostClasses();

// Theme-neutral grey: before storage loads, the user's theme (and App.css with
// its tokens) is not known yet, so the boot skeleton must read on either a dark
// or a light window background.
const BOOT_BLOCK: React.CSSProperties = {
  background: "rgba(128, 128, 128, 0.14)",
  borderRadius: 6,
};

/** App-shaped placeholder painted while `boot()` loads storage, i18n and the
 *  `App` chunk: a sidebar rail with nav rows and a main column with a header
 *  and list rows. Self-contained (inline styles, no i18n) because none of the
 *  app's CSS or translations exist yet. */
function BootSkeleton() {
  const bar = (width: number | string, height: number, extra?: React.CSSProperties) => (
    <div style={{ ...BOOT_BLOCK, width, height, ...extra }} />
  );
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label="Loading"
      style={{ display: "flex", height: "100vh", overflow: "hidden" }}
    >
      <div
        style={{
          width: 220,
          flexShrink: 0,
          padding: "44px 14px 14px",
          display: "flex",
          flexDirection: "column",
          gap: 14,
          borderRight: "1px solid rgba(128, 128, 128, 0.16)",
        }}
      >
        {["70%", "84%", "62%", "76%", "58%", "68%"].map((w, i) => (
          <div key={i}>{bar(w, 12)}</div>
        ))}
      </div>
      <div
        style={{
          flex: 1,
          minWidth: 0,
          padding: "44px 24px 24px",
          display: "flex",
          flexDirection: "column",
          gap: 16,
        }}
      >
        {bar(180, 18)}
        {["62%", "78%", "54%", "70%", "66%", "58%"].map((w, i) => (
          <div key={i} style={{ display: "flex", flexDirection: "column", gap: 8, padding: "6px 0" }}>
            {bar(w, 12)}
            {bar("32%", 9)}
          </div>
        ))}
      </div>
    </div>
  );
}

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);
let appMounted = false;
// Short boots (warm cache) go straight to the app; only a boot still running
// after this delay paints the skeleton, so a fast start never flashes it.
const bootSkeletonTimer = window.setTimeout(() => {
  if (!appMounted) root.render(<BootSkeleton />);
}, 120);

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

  // Invoke timing needs no install step any more: `app/tauriCoreProbe.ts` is
  // aliased over `@tauri-apps/api/core` at build time, so every call site —
  // including the `@tauri-apps/plugin-*` packages — is already wrapped by the
  // time this file runs. The old runtime wrapper could not work at all;
  // `tauriCoreProbe.ts`'s header has the autopsy.

  const { initStorage, migrateFeatureTristate } =
    await import("./storage");

  // Load persisted settings into memory before anything reads them.
  await initStorage();

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

  appMounted = true;
  window.clearTimeout(bootSkeletonTimer);
  root.render(
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

// A failed boot must not leave the skeleton spinning forever: show the error.
boot().catch((e) => {
  console.error("boot failed", e);
  appMounted = true;
  window.clearTimeout(bootSkeletonTimer);
  root.render(
    <div role="alert" style={{ padding: 24, font: "13px system-ui, sans-serif", opacity: 0.8 }}>
      Fleet failed to start: {String(e)}
    </div>,
  );
});
