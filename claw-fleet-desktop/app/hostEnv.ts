/**
 * Which host this bundle is running in, for the forks the transport layer
 * cannot answer.
 *
 * `webTransport.ts` can stand in for a *command*; it cannot stand in for a
 * button that should not be there. Three kinds of UI have to know the host
 * directly:
 *
 *   - entries that only mean something on the desktop (the mobile-relay
 *     pairing panel — a tab has no OS keychain to pair from);
 *   - native-dialog call sites that must swap to the backend-driven picker,
 *     which is the same swap a remote connection already makes;
 *   - the custom-protocol asset URLs (`fleet-attachment://` and friends),
 *     which only resolve inside a Tauri webview.
 *
 * Deliberately a boot-time flag rather than a live `__TAURI_INTERNALS__` probe:
 * `installWebTransport()` installs those internals itself, so a probe run after
 * boot reports "desktop" in the browser build — the exact inversion of what a
 * caller wants. And `?mock` installs the desktop fakes on purpose, so the mock
 * harness must keep reading as the desktop; a probe would flip every mock
 * screenshot to the web layout instead.
 */

let webBuild = false;

/**
 * Called once from the window entry point (`main.tsx`) when
 * that entry decided the page is *not* inside a Tauri webview — i.e. right
 * before it installs the HTTP transport. Nothing else may call this.
 */
export function markWebBuild(): void {
  webBuild = true;
}

/** True when this page is the browser build (`fleet webui`), not the desktop. */
export function isWebBuild(): boolean {
  return webBuild;
}

/**
 * Should the "Mobile" section appear in the navigation?
 *
 * Desktop always has it. The browser build once never showed it, arguing "you're
 * already a remote client, nothing to pair" — that holds for local `fleet webui`,
 * but NOT for cloud deployment. The container runs the same `hooks_server::serve`,
 * which joins the relay itself (`mobile_relay::ensure_ws_client`), so it gets its
 * own pairing code. Scanning it adds one cloud host (with push) to the device roster.
 *
 * The threshold is HTTPS and not loopback, because that's the criterion for "this
 * origin is reachable from a phone": the code contains the relay-hosted page address.
 * A local webui listening only on 127.0.0.1 has no reachable host behind it, so
 * issuing it a pairing code just gives an unreachable device. Plain HTTP is also out
 * — the page on the phone came from HTTPS, and the browser won't let it connect to
 * plain HTTP.
 *
 * Pure function (protocol and hostname come from caller's `window.location`) so these
 * origin judgments can be unit-tested without needing to construct a window.
 */
export function showsMobilePanel(
  webBuildHost: boolean,
  protocol: string,
  hostname: string,
): boolean {
  if (!webBuildHost) return true;
  if (protocol !== "https:") return false;
  return !isLoopbackHostname(hostname);
}

/** Loopback hostnames. `[::1]` is how `location.hostname` represents IPv6 loopback. */
function isLoopbackHostname(hostname: string): boolean {
  const h = hostname.toLowerCase();
  return (
    h === "localhost" ||
    h.endsWith(".localhost") ||
    h === "127.0.0.1" ||
    h.startsWith("127.") ||
    h === "::1" ||
    h === "[::1]"
  );
}
