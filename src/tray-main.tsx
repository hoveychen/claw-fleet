import React from "react";
import ReactDOM from "react-dom/client";

async function boot() {
  const isMockMode =
    new URLSearchParams(window.location.search).has("mock") ||
    import.meta.env.VITE_MOCK === "true";

  if (isMockMode) {
    const { installMocks } = await import("./mock/tauri-mock");
    installMocks();
  }

  try {
    const { initStorage } = await import("./storage");
    await initStorage();
  } catch (e) {
    console.warn("[tray] initStorage failed, proceeding without persisted settings:", e);
  }

  try {
    await import("./i18n");
  } catch (e) {
    console.warn("[tray] i18n init failed:", e);
  }

  const { default: TrayApp } = await import("./TrayApp");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <TrayApp />
    </React.StrictMode>,
  );
}

boot().catch((e) => {
  console.error("[tray] boot failed:", e);
  const root = document.getElementById("root");
  if (root) {
    root.style.cssText = "color:red;padding:8px;font-size:12px;background:rgba(0,0,0,0.8);border-radius:8px;";
    root.textContent = `Tray panel error: ${e}`;
  }
});
