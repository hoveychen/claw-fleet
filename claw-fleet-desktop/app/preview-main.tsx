import React from "react";
import ReactDOM from "react-dom/client";

async function boot() {
  const params = new URLSearchParams(window.location.search);
  const isMockMode = params.has("mock") || import.meta.env.VITE_MOCK === "true";

  if (isMockMode) {
    const { installMocks } = await import("./mock/tauri-mock");
    installMocks();
  }

  const { default: PreviewApp } = await import("./PreviewApp");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <PreviewApp />
    </React.StrictMode>,
  );
}

boot().catch((e) => {
  console.error("[preview] boot failed:", e);
  const root = document.getElementById("root");
  if (root) {
    root.style.cssText = "color:red;padding:8px;font-size:12px;";
    root.textContent = `Preview error: ${e}`;
  }
});
