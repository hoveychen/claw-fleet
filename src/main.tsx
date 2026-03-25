import React from "react";
import ReactDOM from "react-dom/client";

const isMockMode =
  new URLSearchParams(window.location.search).has("mock") ||
  import.meta.env.VITE_MOCK === "true";

async function boot() {
  // In mock mode, install the Tauri API fakes BEFORE anything else loads.
  if (isMockMode) {
    const { installMocks } = await import("./mock/tauri-mock");
    installMocks();
  }

  const { initStorage } = await import("./storage");

  // Load persisted settings into memory before anything reads them.
  await initStorage();

  // i18n must be imported after storage is ready (it reads "lang" synchronously).
  await import("./i18n");

  const { default: App } = await import("./App");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

boot();
