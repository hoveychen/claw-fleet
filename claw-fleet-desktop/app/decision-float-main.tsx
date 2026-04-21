import React from "react";
import ReactDOM from "react-dom/client";

async function boot() {
  try {
    const { initStorage } = await import("./storage");
    await initStorage();
  } catch (e) {
    console.warn("[decision-float] initStorage failed:", e);
  }

  try {
    await import("./i18n");
  } catch (e) {
    console.warn("[decision-float] i18n init failed:", e);
  }

  const { default: DecisionFloatApp } = await import("./DecisionFloatApp");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <DecisionFloatApp />
    </React.StrictMode>,
  );
}

boot().catch((e) => {
  console.error("[decision-float] boot failed:", e);
  const root = document.getElementById("root");
  if (root) {
    root.style.cssText = "color:#f87171;padding:12px;font-size:12px;background:#1a1a1a;";
    root.textContent = `Decision float error: ${e}`;
  }
});
