import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
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

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
