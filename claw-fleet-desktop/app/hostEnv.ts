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

/**
 * 「移动端」板块该不该出现在导航里。
 *
 * 桌面端一直有它。浏览器构建曾经一刀不给,理由是「你已经是远端客户端了,没什么
 * 可配对的」—— 那对本地 `fleet webui` 成立,对**云部署**不成立:那个容器跑的
 * 就是同一个 `hooks_server::serve`,它自己会把自己接进中转
 * (`mobile_relay::ensure_ws_client`),所以它有一张属于自己的配对码,手机扫了
 * 就在聚合设备簿里多一台云主机(带推送)。
 *
 * 门槛是 https 且非回环,因为那正好是「这个 origin 手机也能访问到」的判据:
 * 码里编的是 relay 托管的那个页面地址,但一台只监听 127.0.0.1 的本地 webui
 * 背后没有任何手机能连的主机,给它出码只会得到一台连不上的设备。明文 http
 * 同样出局 —— 手机上那个页面是 https 发的,浏览器不允许它连明文 http。
 *
 * 纯函数(protocol / hostname 由调用方从 `window.location` 取),好让这三种
 * origin 的判定被单测钉住而不必造一个 window。
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

/** 回环主机名。`[::1]` 是 `location.hostname` 给 IPv6 回环的形式。 */
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
