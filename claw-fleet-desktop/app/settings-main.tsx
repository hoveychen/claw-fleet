import React from "react";
import ReactDOM from "react-dom/client";

async function boot() {
  const params = new URLSearchParams(window.location.search);
  const isMockMode = params.has("mock") || import.meta.env.VITE_MOCK === "true";

  if (isMockMode) {
    const { installMocks } = await import("./mock/tauri-mock");
    installMocks();
  }

  const { initStorage } = await import("./storage");
  await initStorage();

  await import("./i18n");

  // Seed the connection store from the URL query param so the Settings window
  // can render "current connection" without pinging the backend.
  const connParam = params.get("connection");
  if (connParam) {
    try {
      const { useConnectionStore } = await import("./store");
      const parsed = JSON.parse(connParam);
      useConnectionStore.getState().setConnection(parsed);
    } catch (e) {
      console.warn("[settings] failed to parse connection param:", e);
    }
  }

  const { default: SettingsApp } = await import("./SettingsApp");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <SettingsApp />
    </React.StrictMode>,
  );
}

boot().catch((e) => {
  console.error("[settings] boot failed:", e);
  const root = document.getElementById("root");
  if (root) {
    root.style.cssText = "color:red;padding:8px;font-size:12px;";
    root.textContent = `Settings error: ${e}`;
  }
});
