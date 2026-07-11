/** Host classes for `<html>`, so CSS / JSX can fork between hosts and platforms.
 *
 * - `tauri-host` — running inside a Tauri webview (a plain browser preview
 *   keeps its default chrome). Mock mode installs the Tauri fakes later in
 *   boot, so the probe correctly reports "not Tauri" at stamp time.
 * - `os-windows` / `os-macos` — Windows draws its own title bar
 *   (`decorations: false`) and needs the chunky WebView2 scrollbar overridden;
 *   macOS keeps the OS traffic lights (`titleBarStyle: Overlay`) and its
 *   native overlay scrollbars.
 *
 * Split into a pure resolver plus a DOM shim so the platform matching is
 * unit-testable without a DOM environment.
 */
export function hostClasses(userAgent: string, hasTauri: boolean): string[] {
  const out: string[] = [];
  if (hasTauri) out.push("tauri-host");
  if (/Windows/i.test(userAgent)) out.push("os-windows");
  else if (/Macintosh|Mac OS X/i.test(userAgent)) out.push("os-macos");
  return out;
}

/** Stamp the host classes onto `<html>`.
 *
 * Every window entry (main, settings, tray, decision-float, preview) must call
 * this synchronously before React mounts — they all import App.css, whose
 * scrollbar and title-bar rules key off these classes, and a late stamp flashes
 * the wrong chrome on first paint.
 */
export function stampHostClasses(): void {
  if (typeof document === "undefined") return;
  const hasTauri =
    typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
  const ua = typeof navigator !== "undefined" ? navigator.userAgent : "";
  document.documentElement.classList.add(...hostClasses(ua, hasTauri));
}
