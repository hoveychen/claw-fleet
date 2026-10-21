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
export function artifactBlobUrl(id: string, name: string, version?: string): string {
  const stand = blobOverride?.(id);
  if (stand) return stand;
  // `version` pins the URL to one entry of the artifact's history; omitted, it
  // means the current version. Passed as a query on both hosts so the `<name>`
  // tail — which is what gives a `<video>` its decoder hint — stays the last
  // path segment either way.
  const pinned = version ? `version=${encodeURIComponent(version)}` : "";
  if (isWebBuild()) {
    const query = `id=${encodeURIComponent(id)}${pinned ? `&${pinned}` : ""}`;
    return `${window.location.origin}/artifact_blob?${query}`;
  }
  const path = `${encodeURIComponent(id)}/${encodeURIComponent(name)}${pinned ? `?${pinned}` : ""}`;
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-artifact.localhost/${path}`
    : `fleet-artifact://localhost/${path}`;
}

/**
 * URL of a video artifact's poster frame, extracted on the host and answered
 * as a JPEG by the same protocol / route as the blob (`poster=1`). 404 when
 * the host has no poster for it — no ffmpeg or Quick Look, or an undecodable
 * file — and the caller falls back.
 *
 * The phone gets the same poster through the relay's `library_thumb`, so all
 * three clients show one frame. `null` under the mock override, which has no
 * host to extract with.
 */
export function artifactPosterUrl(id: string, name: string): string | null {
  if (blobOverride?.(id)) return null;
  if (isWebBuild()) {
    return `${window.location.origin}/artifact_blob?id=${encodeURIComponent(id)}&poster=1`;
  }
  const path = `${encodeURIComponent(id)}/${encodeURIComponent(name)}?poster=1`;
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-artifact.localhost/${path}`
    : `fleet-artifact://localhost/${path}`;
}

let blobOverride: ((id: string) => string | null) | null = null;

/**
 * The `?mock` harness only: answer some artifacts' bytes from inline data
 * URLs. Mock reads as the desktop, whose `fleet-artifact://` scheme a browser
 * tab cannot load, so without this every image fixture would show its icon.
 */
export function setArtifactBlobOverride(fn: ((id: string) => string | null) | null): void {
  blobOverride = fn;
}
