import { isWebBuild } from "./hostEnv";

/**
 * URL of one wiki file through the `fleet-wiki` custom protocol. Built by hand:
 * convertFileSrc() percent-encodes the whole path (`/` → `%2F`), which
 * collapses the URL to a single segment and breaks relative-asset resolution
 * inside HTML docs.
 *
 * In the browser build there is no custom protocol to reach: the scheme is
 * registered on the Tauri webview, and Chromium blocks the navigation outright
 * for an `<iframe src="fleet-wiki://…">` in a tab — measured, and the only
 * trace is one console line, with the doc silently rendering as a blank panel.
 * So the browser gets the same bytes over HTTP from the process that served
 * the page.
 *
 * What it may NOT become there is `/wiki_file?slug=…`, the query route
 * `RemoteBackend` uses: a published `index.html` reaches its bundle through
 * relative refs, which the browser resolves against the URL's *directory*.
 * `/wiki_asset/` keeps every segment where the relative resolution expects it,
 * so `assets/style.css` lands back on the same doc. Only the slug's own `/` is
 * encoded away, so the server's `splitn(3, '/')` still sees slug / version /
 * rel — the same split the desktop protocol handler does.
 *
 * Lives in its own module rather than inside `WikiView` for the same reason as
 * `decisionAssetUrl`: a URL builder with host-dependent behaviour is the part
 * worth unit-testing, and importing it from the component would drag the whole
 * board in.
 */
export function wikiFileUrl(slug: string, version: string, relpath: string): string {
  const path = [slug, version, ...relpath.split("/")]
    .map(encodeURIComponent)
    .join("/");
  if (isWebBuild()) return `${window.location.origin}/wiki_asset/${path}`;
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-wiki.localhost/${path}`
    : `fleet-wiki://localhost/${path}`;
}
