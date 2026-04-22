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

  // Storage init may fail in the overlay window — don't let it block rendering.
  try {
    const { initStorage } = await import("./storage");
    await initStorage();
  } catch (e) {
    console.warn("[overlay] initStorage failed, proceeding without persisted settings:", e);
  }

  try {
    await import("./i18n");
  } catch (e) {
    console.warn("[overlay] i18n init failed:", e);
  }

  try {
    const { installAppContextMenu } = await import("./contextMenu");
    installAppContextMenu();
  } catch (e) {
    console.warn("[overlay] context menu install failed:", e);
  }

  const { default: OverlayApp } = await import("./OverlayApp");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <OverlayApp />
    </React.StrictMode>,
  );
}

boot().catch((e) => {
  console.error("[overlay] boot failed:", e);
  // Render a visible fallback so we can see something went wrong
  const root = document.getElementById("root");
  if (root) {
    root.style.cssText = "color:red;padding:8px;font-size:12px;background:rgba(0,0,0,0.8);border-radius:8px;";
    root.textContent = `Overlay error: ${e}`;
  }
});
