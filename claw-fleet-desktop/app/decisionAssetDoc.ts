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

/**
 * `src` attributes on `<img>` tags in the served document.
 *
 * Only images: they are the documented contract for `fleet__ask`'s `images`
 * (`<img src="chart.png">`), and they are what the auto-gallery emits. A
 * stylesheet or script the agent referenced relatively would still break, but
 * nothing generates those today and inlining arbitrary subresources would mean
 * parsing the document rather than one attribute.
 */
const IMG_SRC_RE = /(<img\b[^>]*?\bsrc\s*=\s*)(["'])([^"']*)\2/gi;

/**
 * A ref that resolves against the asset directory, i.e. one we have to fetch
 * ourselves. Anything with a scheme (`https:`, `data:`, `blob:`), a
 * protocol-relative `//host`, an absolute `/path` or a bare `#frag` is already
 * reachable — or already inline — and is left alone.
 */
export function isRelativeAssetRef(ref: string): boolean {
  return ref.length > 0 && !/^(?:[a-z][a-z0-9+.-]*:|\/\/|\/|#)/i.test(ref);
}

/**
 * Replace every relative `<img src>` in `html` with what `load` returns for it.
 *
 * A ref whose `load` rejects keeps its original attribute: the picture is no
 * worse off than it is today, and one unreachable image must not blank the
 * whole preview.
 */
export async function inlineRelativeImages(
  html: string,
  load: (rel: string) => Promise<string>,
): Promise<string> {
  const refs = new Set<string>();
  for (const m of html.matchAll(IMG_SRC_RE)) {
    if (isRelativeAssetRef(m[3])) refs.add(m[3]);
  }
  if (refs.size === 0) return html;

  const resolved = new Map<string, string>();
  await Promise.all(
    [...refs].map(async (rel) => {
      try {
        resolved.set(rel, await load(rel));
      } catch (err) {
        console.warn(`[decision-asset] inlining ${rel} failed:`, err);
      }
    }),
  );

  return html.replace(IMG_SRC_RE, (whole, pre: string, quote: string, ref: string) => {
    const url = resolved.get(ref);
    return url ? `${pre}${quote}${url}${quote}` : whole;
  });
}

/** Read a fetched image into the `data:` URL the sandboxed frame can load. */
function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("FileReader failed"));
    reader.onload = () => resolve(String(reader.result));
    reader.readAsDataURL(blob);
  });
}

/**
 * The document to hand `AutoHeightFrame`'s `srcDoc` for one card question.
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
  const res = await fetchImpl(decisionAssetUrl(id, qidx, "index.html", theme));
  if (!res.ok) throw new Error(`decision asset index.html → HTTP ${res.status}`);
  const html = await res.text();

  const inlined = await inlineRelativeImages(html, async (rel) => {
    const r = await fetchImpl(decisionAssetUrl(id, qidx, rel));
    if (!r.ok) throw new Error(`${rel} → HTTP ${r.status}`);
    const blob = await r.blob();
    // A gateway that answers 200 with a login page would otherwise be inlined
    // as an `image/…`-shaped data URL and fail silently in the frame.
    if (blob.type.startsWith("text/html")) {
      throw new Error(`${rel} answered HTML, not bytes — an auth gateway intercepted it`);
    }
    return blobToDataUrl(blob);
  });

  return theme ? `${inlined}<style>:root{color-scheme:${theme}}</style>` : inlined;
}
