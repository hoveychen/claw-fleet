/**
 * The browser build's way of showing a decision card's image preview.
 *
 * On the desktop the preview iframe points straight at
 * `fleet-decision://…/index.html` and the relative `<img src="chart.png">` refs
 * inside it resolve against that same directory — no auth, no gateway, done.
 *
 * In the browser build the same document is served over HTTP by the process
 * that served the page, and on a deployment behind a login gateway
 * (`fleet-cloud.muveeai.com` sits behind Traefik ForwardAuth) that costs the
 * images. Measured in Chromium on 2026-09-09 against a minimal repro of the
 * same shape:
 *
 *   GET /                    cookie=YES  sec-fetch-site=none        dest=document
 *   GET /asset/index.html    cookie=YES  sec-fetch-site=same-origin dest=iframe
 *   GET /asset/pic.png       cookie=NO   sec-fetch-site=cross-site  dest=image  → 302 /login
 *
 * The iframe *document* is navigated by the parent, so it counts as same-site
 * and carries the session cookie. Anything the document then requests for
 * itself does not: `sandbox="allow-scripts"` without `allow-same-origin` gives
 * it an opaque origin, which makes its subresource requests cross-site for
 * cookie purposes, so a `SameSite=Lax` session cookie is withheld and the
 * gateway redirects the image to the login page. The card renders its text and
 * caption with a broken-image glyph where the picture should be — exactly the
 * symptom Boss hit on the Teacher's Day card.
 *
 * Dropping the sandbox is not an option (agent-authored HTML would get the
 * app's origin, storage and cookies), so the fetch moves to the side that does
 * have the cookie: the parent page pulls the document *and* its images, and
 * hands the iframe one self-contained `srcDoc` with the images inlined as
 * `data:` URLs.
 *
 * `data:`, not `blob:` — also measured: a `blob:` URL minted by the parent
 * fails to load inside the sandboxed frame, because a blob URL is keyed to its
 * creator's origin and the frame's is opaque. `data:` loads fine.
 */

import { decisionAssetUrl } from "./decisionAssets";
import { fetchSandboxDoc } from "./sandboxDoc";

/**
 * The document to hand `AutoHeightFrame`'s `srcDoc` for one card question.
 *
 * The inlining itself is the shared `sandboxDoc.ts` (images, scripts,
 * stylesheets); this only knows where a card's files live.
 *
 * `theme` cannot ride along as `?theme=` any more — a `srcDoc` document has no
 * URL for core's prelude (`mcp_ipc::THEME_PRELUDE`) to read — so the used
 * colour scheme is pinned with a stylesheet instead. Without it an image-only
 * card follows the OS scheme and paints a white rectangle inside the dark card.
 */
export async function fetchDecisionAssetDoc(
  id: string,
  qidx: string,
  theme?: "dark" | "light",
  fetchImpl: typeof fetch = fetch,
): Promise<string> {
  const inlined = await fetchSandboxDoc(
    "index.html",
    (path) => decisionAssetUrl(id, qidx, path, path === "index.html" ? theme : undefined),
    fetchImpl,
  );
  return theme ? `${inlined}<style>:root{color-scheme:${theme}}</style>` : inlined;
}
