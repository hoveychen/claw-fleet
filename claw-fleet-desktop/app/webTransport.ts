/**
 * Browser build transport — what stands in for Tauri's IPC when this same
 * bundle is opened in a normal browser instead of the desktop webview.
 *
 * The frontend was always a plain web app; `invoke()` was the only thing tying
 * it to Tauri. `mock/liveProxy.ts` already proved the board renders from HTTP
 * alone and already owns the command → route table, so this module does not
 * introduce a second one: it points that table at whatever server delivered
 * this page (same origin, no prefix) and installs it as the IPC handler.
 *
 * That server is `fleet webui`, which serves this bundle and the data routes
 * it calls off one port. (`fleet serve` is the other subcommand — token-gated
 * API only, no bundle — and is not what this talks to.)
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
 *
 * The return *shapes* are load-bearing and are not obvious from the plugin's
 * TypeScript surface: `load` must hand back a numeric resource id and `get` a
 * `[value, exists]` tuple. Answering `null` for both throws nothing — the
 * store's promise simply never settles, `boot()` stops short of
 * `ReactDOM.render`, and the page stays blank with an empty console. That was
 * the first version of this function; `webTransport.test.ts` now pins each
 * shape. `mock/tauri-mock.ts` is the only other written record of them.
 */
export function localCommand(cmd: string, args: Record<string, unknown>): { handled: boolean; value?: unknown } {
  const key = () => `${STORE_PREFIX}${String(args.key ?? "")}`;
  switch (cmd) {
    // Resource id — a number is expected, and `null` wedges the boot.
    case "plugin:store|load":
      return { handled: true, value: 1 };
    case "plugin:store|save":
    case "plugin:store|clear":
    case "plugin:store|reset":
      return { handled: true, value: null };
    case "plugin:store|get": {
      const value = window.localStorage.getItem(key());
      return { handled: true, value: [value, value !== null] };
    }
    case "plugin:store|set":
      window.localStorage.setItem(key(), String(args.value));
      return { handled: true, value: null };
    case "plugin:store|delete":
      window.localStorage.removeItem(key());
      return { handled: true, value: null };
    case "plugin:store|entries":
    case "plugin:store|keys":
    case "plugin:store|values":
      return { handled: true, value: [] };
    case "plugin:store|length":
      return { handled: true, value: 0 };
    case "plugin:store|has":
      return { handled: true, value: false };

    // Host facts the desktop reads from the OS. The browser build reports
    // itself as such rather than impersonating a platform.
    case "get_platform":
      return { handled: true, value: "web" };
    case "get_app_version":
    case "desktop_build_commit":
      return { handled: true, value: "web" };

    // SSH connection management. The front door only ever talks to the backend
    // inside the app that served this page, so there is nothing to list — but
    // the answer must still be a *list*: `ConnectionDialog` reads `.length` off
    // it during render, and a `null` there throws mid-render and leaves the
    // whole tree unmounted (root stays empty, no console error, `boot()` still
    // resolves). That is the failure this pair exists to prevent.
    case "list_saved_connections":
    case "list_ssh_profiles":
      return { handled: true, value: [] };

    // Everything else is left unhandled and rejected by the caller below.
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

    // Reject rather than resolve `null`.
    //
    // `null` looks like the harmless choice and is the opposite: a caller that
    // does `invoke<T[]>(...).then(setState)` stores the null, and the next
    // render throws on `.length` — mid-render, so React unmounts the whole
    // tree and the page goes blank with `boot()` still reporting success.
    // Rejecting instead lands in the caller's `catch`, or surfaces as an
    // unhandled rejection, and either way leaves its state at the correctly
    // typed initial value. Noisy beats blank, and these gaps should be seen.
    if (!gaps.has(cmd)) {
      gaps.add(cmd);
      console.warn(`[web] no transport for "${cmd}" — rejecting`);
    }
    throw new Error(`command "${cmd}" is not available in the browser build`);
  }, { shouldMockEvents: true });

  // Installs the board poller. The desktop pushes `sessions-updated` over an
  // app event; over HTTP the page has to ask.
  installLiveProxy();

  (window as unknown as Record<string, unknown>).__webTransportGaps = webTransportGaps;
  (window as unknown as Record<string, unknown>).__liveProxyReport = liveProxyReport;
}
