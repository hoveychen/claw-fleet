import React from "react";
import ReactDOM from "react-dom/client";

const params = new URLSearchParams(window.location.search);
const isMockMode = params.has("mock") || import.meta.env.VITE_MOCK === "true";
const forceLite = params.has("lite");

// Tag the document root with host classes so CSS / JSX can fork between
// macOS (titleBarStyle: Overlay — OS keeps traffic lights) and Windows
// (decorations: false — we draw the whole title bar). Mock mode skips
// `tauri-host` so a plain browser preview keeps default chrome.
{
  const cls = document.documentElement.classList;
  const hasTauri =
    typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
  if (hasTauri) cls.add("tauri-host");
  const ua = typeof navigator !== "undefined" ? navigator.userAgent : "";
  if (/Windows/i.test(ua)) cls.add("os-windows");
  else if (/Macintosh|Mac OS X/i.test(ua)) cls.add("os-macos");
}

async function boot() {
  // In mock mode, install the Tauri API fakes BEFORE anything else loads.
  if (isMockMode) {
    const { installMocks } = await import("./mock/tauri-mock");
    installMocks();
  }

  const { initStorage, setItem } = await import("./storage");

  // Load persisted settings into memory before anything reads them.
  await initStorage();

  // `?lite` — pre-flip the lite-mode flag so UIStore picks it up at construction.
  // Mock-only shortcut so we can iterate on the portrait UI without tauri dev.
  if (forceLite) {
    setItem("liteMode", "true");
  }

  // i18n must be imported after storage is ready (it reads "lang" synchronously).
  await import("./i18n");

  const { installAppContextMenu } = await import("./contextMenu");
  installAppContextMenu();

  const { default: App } = await import("./App");

  // In mock mode, auto-accept a local connection so the ConnectionDialog
  // doesn't block the layout we want to iterate on.
  if (isMockMode) {
    const { useConnectionStore } = await import("./store");
    useConnectionStore.getState().setConnection({ type: "local" });
  }

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

boot();
