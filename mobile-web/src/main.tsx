import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { CloudApp } from "./cloud/CloudApp";
import { initTheme } from "./theme";
import { initWakeLock } from "./wakeLock";
import { lockZoom } from "./lockZoom";
import "./index.css";

initTheme();
initWakeLock();
lockZoom();

const cloudMode = import.meta.env.MODE === "cloud";

if (!cloudMode && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch(() => {
      // dev over plain http (non-localhost) has no SW; the app still works
    });
  });
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {cloudMode ? <CloudApp /> : <App />}
  </StrictMode>,
);
