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

import {
  liveInvoke,
  installLiveProxy,
  setProbeBase,
  setHostPrefsSource,
  liveProxyReport,
} from "./mock/liveProxy";

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
    // Snake_case on purpose — `App.tsx` reads `has_update` off this shape.
    case "check_app_version":
      return {
        handled: true,
        value: { has_update: false, latest_version: "", release_url: "" },
      };
    // The desktop log lives on the host; a tab has no path to show.
    case "get_log_path":
      return { handled: true, value: "" };

    // ── Native window / tray / dock ──────────────────────────────────────
    // There is one window here and the browser owns its chrome, so these are
    // no-ops rather than gaps: the callers fire and forget, and reporting a
    // gap for each would bury the ones that matter.
    case "show_main_window":
    case "open_settings_window":
    case "open_preview_window":
    case "close_preview_window":
    case "show_decision_float":
    case "hide_decision_float":
    case "resize_decision_float":
    case "toggle_tray_panel":
    case "set_lite_mode":
    case "quit_app":
    case "nudge_traffic_lights":
      return { handled: true, value: null };
    case "is_main_window_minimized":
      return { handled: true, value: false };
    // The float window does not exist here, so it holds no snapshot. Typed
    // `PendingDecision[] | null` by its caller, so null is in-contract.
    case "get_decision_float_snapshot":
      return { handled: true, value: null };
    // `app.restart()`'s honest browser equivalent.
    case "restart_app":
      window.location.reload();
      return { handled: true, value: null };
    // …and the print sheet really is the same feature.
    case "print_webview":
      window.print();
      return { handled: true, value: null };
    // Opening a file manager / the OS notification pane needs the host shell.
    case "reveal_path":
    case "open_notification_settings":
      return { handled: true, value: null };

    // ── Host-side prefs the frontend itself owns ─────────────────────────
    // Both are pushed *from* the frontend (which persists them in this same
    // store), so recording them is the whole job on this side. The desktop
    // additionally re-applies every installed guidance carrier on each call;
    // that side effect is not reproduced here — the Settings panel's explicit
    // apply buttons cover it, and they now carry these two values (see
    // `setHostPrefsSource`).
    case "set_locale":
    case "set_user_title":
    case "set_notification_mode":
      return { handled: true, value: null };

    // ── Native dialogs ──────────────────────────────────────────────────
    // `null` is this pair's "user cancelled", which is the truthful answer
    // when there is no dialog to open.
    case "pick_file":
    case "save_skill_file":
      return { handled: true, value: null };

    // ── Host audio / OS TTS ─────────────────────────────────────────────
    // The desktop synthesises through edge-tts on the host. Answer with an
    // empty voice list (list-shaped — `audio.ts` maps over it) and swallow the
    // speak call rather than substituting a different engine's voices.
    case "get_tts_voices":
      return { handled: true, value: [] };
    case "speak_text":
      return { handled: true, value: null };

    // Local-CLI detection and the mascot's LLM quips both run on the host with
    // no route to reach them. Both are read as collections, so they answer as
    // empty collections instead of null.
    case "detect_ai_tools":
      return { handled: true, value: [] };
    case "generate_mascot_quips":
      return { handled: true, value: { busy: [], idle: [] } };

    // Keep-awake holds a power assertion on the host; a tab cannot.
    case "keep_awake_supported":
    case "get_keep_awake":
    case "set_keep_awake":
      return { handled: true, value: false };

    // In-memory on the desktop, fed by its own session watcher. Nothing here
    // populates it — and `store.ts` assigns the result straight into an array
    // slot, so this must be a list, never null.
    case "get_waiting_alerts":
      return { handled: true, value: [] };

    // The desktop pre-flights `X-Frame-Options` so it can explain a blank
    // iframe before showing one. A page cannot read another origin's headers,
    // and `WebTabPane` already treats `null` as "this host doesn't know the
    // command" and fails open — so answer null and let the iframe speak.
    case "probe_url_embeddable":
      return { handled: true, value: null };

    // Console instead of the host log file.
    case "log_frontend_debug":
      console.debug("[frontend]", args.msg);
      return { handled: true, value: null };

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
    //
    // Deliberately *not* given a stand-in value, because any value would be a
    // lie the UI would present as fact:
    //   - get/set_permissions_config, get/set_decision_panel_config,
    //     get/set_auto_resume_config — host settings under `~/.fleet` with no
    //     endpoint on `fleet serve` (see `hooks_server`: `/auto_resume_config`
    //     was retired in Phase 4 P3 and the other two never had one). A
    //     fabricated "current value" would show a toggle that reads as the
    //     host's state and saves nowhere. Each caller already has a `.catch`
    //     and a typed initial value, so rejecting degrades cleanly. Wiring
    //     these needs three new routes, not a frontend change.
    //   - get_claude_md_content, promote_memory — read/write a workspace file
    //     through `memory::` directly instead of the Backend trait, so they
    //     have no HTTP shape to mirror.
    //   - install_fleet_cli, install_fleet_skill, apply_mcp_injector — write to
    //     the *caller's* machine.
    //   - stage_pasted_attachment, upload_elicitation_attachment,
    //     export_wiki_doc — move bytes through a caller-side filesystem path.
    //   - test_decision_frontend_only, test_fleet_ask_* — emit onto the
    //     desktop's own app-event bus / have no RemoteBackend override.
    //   - connect_remote, disconnect_remote, delete_connection,
    //     install_rca_remote, update_rca_remote — SSH connection management.
    //     The tab only ever talks to the server that served it.
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

  // The guidance routes (`/apply_interaction_mode`, `/apply_prd_mode`, …) need
  // the user's title and locale in their body. On the desktop those live in
  // AppState; here they live in this store, under the same keys `storage.ts`
  // registers ("user-title", "lang"). Read on each call rather than cached, so
  // an apply right after a language switch carries the new value.
  setHostPrefsSource(() => ({
    userTitle: window.localStorage.getItem(`${STORE_PREFIX}user-title`) ?? "",
    locale: window.localStorage.getItem(`${STORE_PREFIX}lang`) ?? "en",
  }));

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
