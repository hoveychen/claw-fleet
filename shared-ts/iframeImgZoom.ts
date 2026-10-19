// Parent side of the click-to-zoom bridge for images inside a sandboxed
// <iframe> (decision-card previews, wiki docs). The frame posts
// `{ __fleetImgZoom: <src> }` — from `mcp_ipc::IMG_ZOOM_SCRIPT` (core) or the
// phone's own `IMG_ZOOM_INJECT` — and the parent opens its lightbox.

const ZOOM_KEY = "__fleetImgZoom";

// What a lightbox may load. The payload comes from agent-authored html, so the
// parent never hands an arbitrary URL scheme to its own <img>/fetch.
const ALLOWED = /^(data:image\/|blob:|https?:|fleet-decision:|fleet-wiki:)/i;

/** Pull the image src out of a postMessage payload, or null if it isn't a
 *  zoom message. Validate the shape — the payload crosses a trust boundary. */
export function parseImgZoom(data: unknown): string | null {
  if (!data || typeof data !== "object") return null;
  const src = (data as Record<string, unknown>)[ZOOM_KEY];
  return typeof src === "string" && ALLOWED.test(src) ? src : null;
}
