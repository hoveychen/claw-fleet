import { isWebBuild } from "./hostEnv";

/**
 * URL of one artifact's blob through the `fleet-artifact` custom protocol.
 *
 * The `<name>` tail is cosmetic — the id alone identifies the blob — but it is
 * load-bearing for the viewer: a `<video>` picks its decoder and a PDF frame
 * decides to render partly from the extension on the URL. Serving everything
 * as `/<id>` would hand every artifact the same nameless URL.
 *
 * Both segments are `encodeURIComponent`'d, so a name with a space or a CJK
 * character survives, and the host splits on the first `/` to recover the id.
 *
 * In the browser build there is no custom protocol to reach — the scheme is
 * registered on the Tauri webview, and Chromium blocks the navigation for an
 * `<img>`/`<video>` in a tab (the same silent failure `wikiFileUrl` documents).
 * The browser gets the same bytes over HTTP from the process that served the
 * page, and `/artifact_blob` honours `Range` exactly like the protocol does,
 * so seeking works on both.
 *
 * Its own module, like `wikiAssets` and `decisionAssets`: a URL builder with
 * host-dependent behaviour is the part worth unit-testing, and importing it
 * from the view would drag the whole board into the test.
 */
export function artifactBlobUrl(id: string, name: string): string {
  if (isWebBuild()) {
    return `${window.location.origin}/artifact_blob?id=${encodeURIComponent(id)}`;
  }
  const path = `${encodeURIComponent(id)}/${encodeURIComponent(name)}`;
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-artifact.localhost/${path}`
    : `fleet-artifact://localhost/${path}`;
}
