import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App, type TransportFactory } from "./App";
import { CloudApp } from "./cloud/CloudApp";
import { LightboxProvider } from "./views/Lightbox";
import { ErrorBoundary } from "./ErrorBoundary";
import { t } from "./i18n";
import { initTheme } from "./theme";
import { initWakeLock } from "./wakeLock";
import { installNativeWakeLock } from "./wakeLockNative";
import { lockZoom } from "./lockZoom";
import { ConfirmProvider } from "./confirmDialog";
import { Skeleton, SkeletonCard } from "./views/loading";
import "./index.css";

initTheme();
initWakeLock();
// When the shell lacks standard wakeLock, add native fallback (iOS 18.4 and below). Async, fails silently:
// After installation, wakeLock module re-aligns lock state itself.
void installNativeWakeLock();
lockZoom();

const cloudMode = import.meta.env.MODE === "cloud";

if (!cloudMode && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    // Use BASE_URL instead of hardcoding "/": same-origin form is under `/m/`, registering `/sw.js` tries
    // to fetch a file that doesn't exist in root, SW silently fails to activate.
    navigator.serviceWorker.register(`${import.meta.env.BASE_URL}sw.js`).catch(() => {
      // dev over plain http (non-localhost) has no SW; the app still works
    });
  });
}

// The whole 'no relay in same-origin builds' rule hinges on this dynamic import pair. `IS_WEBUI` is a
// compile-time constant (see hostMode.ts), so Rollup drops the unused branch along with its entire
// dependency tree — relay client, encryption, pairing storage, none of it enters the webui build.
// If we switched to runtime checks, both sides would be bundled, which is what we're avoiding.
// Write `import.meta.env.VITE_FLEET_HOST` directly in the condition, not hostMode's IS_WEBUI:
// Vite's define only replaces the literal expression, and after folding to `"webui" === "webui"`
// Rollup must drop the other branch. When routed through hostMode's const layer it won't drop —
// verified: dist-webui still outputs a relay-*.js chunk (can find resolveRelayBase and
// fleet-relay/hkdf/v1). IS_WEBUI works fine elsewhere, only this spot is sensitive to folding.
const root = createRoot(document.getElementById("root")!);

// Cold start: the transport chunk below is a top-level await, and until it lands
// #root would stay blank. Paint the app's rough shape (header + a few cards)
// meanwhile; the real tree replaces it in the same root.
root.render(
  <div
    style={{
      display: "flex",
      flexDirection: "column",
      gap: 10,
      padding: "calc(10px + var(--safe-top, 0px)) 12px 12px",
    }}
  >
    <Skeleton height={24} width="40%" style={{ marginBottom: 8 }} />
    {[0, 1, 2].map((i) => (
      <SkeletonCard key={i} height={96} />
    ))}
  </div>,
);

const { makeTransport }: { makeTransport: TransportFactory } =
  import.meta.env.VITE_FLEET_HOST === "webui"
    ? await import("./transportWebui")
    : await import("./transportRelay");

root.render(
  <StrictMode>
    {/* Outermost error boundary. Inside are two finer layers (per tab, per decision card); this layer
        catches what they don't surround — the app shell itself, various overlays, and cloud forms. Without it,
        exceptions at those positions become blank white page + empty console, impossible to diagnose on mobile.
        resetKey not passed: if root crashes, there's no 'retry card', user clicks retry or reopens. */}
    <ErrorBoundary label={t("Fleet")}>
      <ConfirmProvider>
        {cloudMode ? (
          <CloudApp />
        ) : (
          <LightboxProvider>
            <App makeTransport={makeTransport} />
          </LightboxProvider>
        )}
      </ConfirmProvider>
    </ErrorBoundary>
  </StrictMode>,
);
