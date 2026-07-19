import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { HostedApp } from "./cloud/HostedApp";
import { EmbedApp } from "./embed/EmbedApp";
import { initTheme } from "./theme";
import { initWakeLock } from "./wakeLock";
import { lockZoom } from "./lockZoom";
import "./index.css";

initTheme();
initWakeLock();
lockZoom();

if ("serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch(() => {
      // dev over plain http (non-localhost) has no SW; the app still works
    });
  });
}

const path = location.pathname.split("/").filter(Boolean);
const apiBaseUrl = import.meta.env.VITE_CLOUD_API_URL || "/api/v1";
const requestedEmbedView = new URLSearchParams(location.search).get("view");
const embedView = requestedEmbedView === "decision_card" || requestedEmbedView === "usage" || requestedEmbedView === "decision_inbox" || requestedEmbedView === "task_detail"
  ? requestedEmbedView
  : path[1] === "task" ? "task_detail" : "decision_inbox";
const content = path[0] === "embed"
  ? <EmbedApp apiBaseUrl={apiBaseUrl} taskId={path[1] === "task" ? path[2] : undefined} view={embedView} />
  : (path[0] === "project" || new URLSearchParams(location.search).get("mode") === "cloud")
    ? <HostedApp apiBaseUrl={apiBaseUrl} /> : <App />;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {content}
  </StrictMode>,
);
