/**
 * Browser build transport — what stands in for Tauri's IPC when this same
 * bundle is opened in a normal browser instead of the desktop webview.
 *
 * The frontend was always a plain web app; `invoke()` was the only thing tying
 * it to Tauri. `mock/liveProxy.ts` already proved the board renders from HTTP
 * alone and already owns the command → route table, so this module does not
 * introduce a second one: it points that table at the app's own front door
 * (`web_serve.rs`, same origin, no prefix) and installs it as the IPC handler.
 *
 * Deliberately *not* a separate entry point or build: the shipped `dist/` is
 * the same one Tauri bundles, and which transport gets installed is decided at
 * runtime by whether `__TAURI_INTERNALS__` is present.
 */

import { liveInvoke, installLiveProxy, setProbeBase, liveProxyReport } from "./mock/liveProxy";

/**
 * True when running inside the desktop webview.
 *
 * Tauri injects `__TAURI_INTERNALS__` before any app code runs, so its absence
 * means a plain browser. Checked rather than a build flag so one bundle serves
 * both hosts.
 */
export function isTauriHost(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** localStorage key prefix for the settings store. */
const STORE_PREFIX = "fleet-web:";

/**
 * Commands with no HTTP route, answered locally instead.
 *
 * `plugin:store` is the app's settings persistence and runs on the boot path —
 * without it `initStorage()` never resolves and nothing renders. localStorage
 * is the natural stand-in: same per-origin, per-device semantics the desktop
 * store has.
 */
function localCommand(cmd: string, args: Record<string, unknown>): { handled: boolean; value?: unknown } {
  const key = () => `${STORE_PREFIX}${String(args.key ?? "")}`;
  switch (cmd) {
    case "plugin:store|load":
    case "plugin:store|save":
    case "plugin:store|clear":
    case "plugin:store|reset":
      return { handled: true, value: null };
    case "plugin:store|get":
      return { handled: true, value: window.localStorage.getItem(key()) };
    case "plugin:store|set":
      window.localStorage.setItem(key(), String(args.value));
      return { handled: true, value: null };
    case "plugin:store|delete":
      window.localStorage.removeItem(key());
      return { handled: true, value: null };
    case "plugin:store|entries":
    case "plugin:store|keys":
      return { handled: true, value: [] };

    // Host facts the desktop reads from the OS. The browser build reports
    // itself as such rather than impersonating a platform.
    case "get_platform":
      return { handled: true, value: "web" };
    case "get_app_version":
    case "desktop_build_commit":
      return { handled: true, value: "web" };

    // Window / tray / process control has no browser equivalent. Answering
    // `null` keeps callers that ignore the result working; the gap list below
    // records anything that turns out to need more than that.
    default:
      return { handled: false };
  }
}

/** Commands that reached neither the probe nor a local answer, deduped. */
const gaps = new Set<string>();

/** Readable from the console — the list of commands still unanswered. */
export function webTransportGaps(): string[] {
  return [...gaps].sort();
}

export async function installWebTransport(): Promise<void> {
  const { mockIPC, mockWindows } = await import("@tauri-apps/api/mocks");

  // Sets up `__TAURI_INTERNALS__.metadata`, which the event and window APIs
  // read before any handler runs — must come first.
  mockWindows("main");

  // Same origin: this page was served by the very process that answers the
  // data routes, so no proxy prefix.
  setProbeBase("");

  // `shouldMockEvents` routes `emit`/`listen` through the same handler, which
  // is what lets liveProxy's pollers stand in for the desktop's push channels
  // (`sessions-updated`, `session-tail`).
  mockIPC(async (cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>;

    const live = await liveInvoke(cmd, a);
    if (live.handled) return live.value;

    const local = localCommand(cmd, a);
    if (local.handled) return local.value;

    if (!gaps.has(cmd)) {
      gaps.add(cmd);
      console.warn(`[web] no transport for "${cmd}" — returned null`);
    }
    return null;
  }, { shouldMockEvents: true });

  // Installs the board poller. The desktop pushes `sessions-updated` over an
  // app event; over HTTP the page has to ask.
  installLiveProxy();

  (window as unknown as Record<string, unknown>).__webTransportGaps = webTransportGaps;
  (window as unknown as Record<string, unknown>).__liveProxyReport = liveProxyReport;
}
