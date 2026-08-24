/**
 * Which host this bundle is running in, for the forks the transport layer
 * cannot answer.
 *
 * `webTransport.ts` can stand in for a *command*; it cannot stand in for a
 * button that should not be there. Three kinds of UI have to know the host
 * directly:
 *
 *   - entries that only mean something on the desktop (the Lite portrait
 *     window, the mobile-relay pairing panel — a tab has no second window to
 *     shrink and no OS keychain to pair from);
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
 * Called once from a window entry point (`main.tsx`, `settings-main.tsx`) when
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
