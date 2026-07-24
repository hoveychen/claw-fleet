import React from "react";
import ReactDOM from "react-dom/client";
import { stampHostClasses } from "./hostClass";
import { primePromoStorage, promoSceneFromSearch } from "./mock/promo-scene";

const params = new URLSearchParams(window.location.search);
const isMockMode = params.has("mock") || import.meta.env.VITE_MOCK === "true";
const forceLite = params.has("lite");
const mockQaMode = params.has("qa");
const promoScene = promoSceneFromSearch(window.location.search);

if (isMockMode && promoScene) {
  primePromoStorage(window.localStorage);
}

stampHostClasses();

async function boot() {
  let triggerPromoScene: ((scene: NonNullable<typeof promoScene>) => void) | null = null;
  let triggerMockQaScenario: (() => void) | null = null;
  // In mock mode, install the Tauri API fakes BEFORE anything else loads.
  if (isMockMode) {
    const mocks = await import("./mock/tauri-mock");
    const { installMocks } = mocks;
    installMocks({ qaMode: mockQaMode });
    triggerPromoScene = mocks.triggerPromoScene;
    triggerMockQaScenario = mocks.triggerMockQaScenario;
  }

  const { initStorage, setItem, migrateSessionViewDefault } = await import("./storage");

  // Load persisted settings into memory before anything reads them.
  await initStorage();

  // Roll out gallery as the default session view for existing users whose disk
  // still carries a stale "list". Must run before the UIStore is constructed
  // (i.e. before ./App is imported below), same window as the ?lite pre-flip.
  migrateSessionViewDefault();

  // `?lite` — pre-flip the lite-mode flag so UIStore picks it up at construction.
  // Mock-only shortcut so we can iterate on the portrait UI without tauri dev.
  if (forceLite) {
    setItem("liteMode", "true");
  }

  // i18n must be imported after storage is ready (it reads "lang" synchronously).
  await import("./i18n");

  const { installAppContextMenu } = await import("./contextMenu");
  installAppContextMenu();

  const { default: App } = await import("./App");

  // In mock mode, auto-accept a local connection so the ConnectionDialog
  // doesn't block the layout we want to iterate on.
  //
  // `?mock&remote` seeds a remote connection instead — the only way to exercise
  // the UI that branches on `isRemote` (e.g. the launcher's directory picker,
  // which swaps the native OS dialog for the backend-driven one because the
  // native dialog can only browse this desktop's disk).
  if (isMockMode) {
    const { useConnectionStore } = await import("./store");
    useConnectionStore.getState().setConnection(
      params.has("remote")
        ? {
            type: "remote",
            connection: {
              id: "mock-probe",
              label: "build-box",
              host: "build-box.internal",
              port: 22,
              username: "demo",
              identityFile: null,
              jumpHost: null,
              sshProfile: null,
            },
          }
        : { type: "local" },
    );
  }

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );

  if (promoScene && triggerPromoScene) {
    window.setTimeout(() => triggerPromoScene?.(promoScene), 900);
  }
  if (mockQaMode && triggerMockQaScenario) {
    window.setTimeout(() => triggerMockQaScenario?.(), 900);
  }
}

boot();
