/**
 * URL of one wiki file through the `fleet-wiki` custom protocol. Built by hand:
 * convertFileSrc() percent-encodes the whole path (`/` → `%2F`), which
 * collapses the URL to a single segment and breaks relative-asset resolution
 * inside HTML docs.
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
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-wiki.localhost/${path}`
    : `fleet-wiki://localhost/${path}`;
}
