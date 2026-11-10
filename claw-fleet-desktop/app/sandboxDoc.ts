/**
 * Turn a served HTML document plus its relative subresources into one
 * self-contained string for a sandboxed iframe's `srcDoc`.
 *
 * Every agent-authored HTML preview (decision cards, wiki HTML docs, artifact
 * HTML) renders in `sandbox="allow-scripts"` *without* `allow-same-origin`, so
 * the document gets an opaque origin. Two hosts cannot then let the frame load
 * anything by itself:
 *
 *   - Behind a login gateway (`fleet-cloud`), the frame's subresource requests
 *     are cross-site for cookie purposes and the `SameSite=Lax` session cookie
 *     is withheld — images come back as a redirect to /login. Measured in
 *     Chromium on 2026-09-09; see `decisionAssetDoc.ts`.
 *   - Served from a relay under `/desktop/` (`relay/tunnel.ts`), data only
 *     exists inside the page's E2E socket and the Service Worker that bridges
 *     to it. Measured in headless Chromium on 2026-11-10: an opaque-origin
 *     sandboxed frame's navigation is *not* handled by the SW at all — it goes
 *     straight to the relay, which answers with mobile-web's fallback page.
 *
 * Loosening the sandbox is not an option, so the parent (which has the cookie
 * and the socket) fetches the document and its relative `<img>`, `<source>`,
 * `<script>` and stylesheet / icon `<link>` refs, plus `url()` refs inside
 * `<style>` blocks and fetched stylesheets, and rewrites each to a `data:` URL.
 *
 * `data:`, not `blob:`: a blob URL is keyed to its creator's origin, and the
 * sandboxed frame's origin is opaque, so a parent-minted blob URL fails to load
 * inside it (measured with the decision-card fix).
 *
 * What this cannot cover, by construction: refs the page's own JS builds at
 * runtime (`fetch("data.json")`, dynamic `import()`), CSS `@import`, `srcset`,
 * and navigations to sibling pages (`<a href="other.html">`). Those still need
 * a frame with a real URL.
 */

/**
 * A ref that resolves against the document's directory, i.e. one we have to
 * fetch ourselves. Anything with a scheme (`https:`, `data:`, `blob:`), a
 * protocol-relative `//host`, an absolute `/path` or a bare `#frag` is already
 * reachable — or already inline — and is left alone.
 */
export function isRelativeAssetRef(ref: string): boolean {
  return ref.length > 0 && !/^(?:[a-z][a-z0-9+.-]*:|\/\/|\/|#)/i.test(ref);
}

/**
 * Resolve a relative `ref` against the file at `basePath` (both relative to
 * the doc root, `/`-separated) into a decoded root-relative path.
 *
 * Query and fragment are dropped — every store we read from is a plain file
 * tree. Percent-escapes are decoded because callers re-encode each segment
 * when they build the fetch URL. `null` when the ref climbs above the root:
 * there is nothing of ours to fetch there.
 */
export function resolveAssetPath(basePath: string, ref: string): string | null {
  const bare = ref.replace(/[?#].*$/, "");
  if (bare === "") return null;
  const out = basePath.split("/").slice(0, -1);
  for (const raw of bare.split("/")) {
    let seg: string;
    try {
      seg = decodeURIComponent(raw);
    } catch {
      seg = raw;
    }
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (out.length === 0) return null;
      out.pop();
      continue;
    }
    out.push(seg);
  }
  return out.length > 0 ? out.join("/") : null;
}

/** Fetch one root-relative file's bytes; rejecting leaves its ref untouched. */
export type AssetLoader = (path: string) => Promise<Blob>;

/** What a fetched file is about to become, which fixes its `data:` MIME. */
type AssetKind = "image" | "script" | "style";

const TAG_RE = /<(img|source|script|link)\b[^>]*>/gi;
const STYLE_BLOCK_RE = /(<style\b[^>]*>)([\s\S]*?)(<\/style\s*>)/gi;
const CSS_URL_RE = /url\(\s*(?:(["'])([^"')]*)\1|([^"')\s]+))\s*\)/gi;

/**
 * `name=value` inside one tag. The leading whitespace is required so `src`
 * does not match inside `data-src`. Unquoted values count: agents write
 * `<img src=chart.png>` as often as the quoted form.
 */
function attrRe(name: string): RegExp {
  return new RegExp(`(\\s${name}\\s*=\\s*)(?:(["'])([^"']*)\\2|([^\\s"'=<>\`]+))`, "i");
}
const SRC_ATTR = attrRe("src");
const HREF_ATTR = attrRe("href");
const REL_ATTR = attrRe("rel");

async function replaceAsync(
  input: string,
  re: RegExp,
  fn: (m: RegExpMatchArray) => Promise<string>,
): Promise<string> {
  const parts: Array<string | Promise<string>> = [];
  let last = 0;
  for (const m of input.matchAll(re)) {
    const at = m.index ?? 0;
    parts.push(input.slice(last, at), fn(m));
    last = at + m[0].length;
  }
  parts.push(input.slice(last));
  return (await Promise.all(parts)).join("");
}

/** Read bytes into a `data:` URL with the given MIME (blob types lie). */
function blobToDataUrl(blob: Blob, type: string): Promise<string> {
  const typed = new Blob([blob], { type });
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("FileReader failed"));
    reader.onload = () => resolve(String(reader.result));
    reader.readAsDataURL(typed);
  });
}

class Inliner {
  private memo = new Map<string, Promise<string>>();

  constructor(private load: AssetLoader) {}

  /** `ref` as seen from `basePath`, as a `data:` URL — or `null` to keep it. */
  async dataUrl(basePath: string, ref: string, kind: AssetKind): Promise<string | null> {
    if (!isRelativeAssetRef(ref)) return null;
    const path = resolveAssetPath(basePath, ref);
    if (path === null) return null;
    const key = `${kind}\u0000${path}`;
    let p = this.memo.get(key);
    if (!p) {
      p = this.fetchAs(path, kind);
      this.memo.set(key, p);
    }
    try {
      return await p;
    } catch (err) {
      // One unreachable file must not blank the whole preview: the ref keeps
      // its original value, no worse off than before.
      console.warn(`[sandbox-doc] inlining ${path} failed:`, err);
      return null;
    }
  }

  private async fetchAs(path: string, kind: AssetKind): Promise<string> {
    const blob = await this.load(path);
    // A gateway that answers 200 with a login page would otherwise be inlined
    // as an image/script and fail silently in the frame.
    if (blob.type.startsWith("text/html")) {
      throw new Error(`${path} answered HTML, not the asset — an auth gateway intercepted it`);
    }
    if (kind === "style") {
      const css = await this.rewriteCss(await blob.text(), path);
      return `data:text/css;charset=utf-8,${encodeURIComponent(css)}`;
    }
    if (kind === "script") return blobToDataUrl(blob, "text/javascript");
    return blobToDataUrl(blob, blob.type || "application/octet-stream");
  }

  /** Inline the `url()` refs in `css`, which lives at `cssPath`. */
  rewriteCss(css: string, cssPath: string): Promise<string> {
    return replaceAsync(css, CSS_URL_RE, async (m) => {
      const url = await this.dataUrl(cssPath, m[2] ?? m[3], "image");
      return url ? `url("${url}")` : m[0];
    });
  }

  /** Rewrite one attribute inside `tag`, or return the tag unchanged. */
  async rewriteAttr(tag: string, attr: RegExp, docPath: string, kind: AssetKind): Promise<string> {
    const m = tag.match(attr);
    if (!m) return tag;
    const url = await this.dataUrl(docPath, m[3] ?? m[4], kind);
    if (!url) return tag;
    // An unquoted ref comes back double-quoted: a data: URL may hold
    // characters an unquoted attribute value cannot.
    const q = m[2] ?? '"';
    return tag.replace(attr, `${m[1]}${q}${url}${q}`);
  }
}

/**
 * Replace every relative subresource ref in `html` (which lives at `docPath`,
 * relative to the doc root) with a `data:` URL built from `load`.
 */
export async function inlineDocAssets(
  html: string,
  docPath: string,
  load: AssetLoader,
): Promise<string> {
  const inliner = new Inliner(load);
  const withStyles = await replaceAsync(html, STYLE_BLOCK_RE, async (m) => {
    return `${m[1]}${await inliner.rewriteCss(m[2], docPath)}${m[3]}`;
  });
  return replaceAsync(withStyles, TAG_RE, async (m) => {
    const tag = m[0];
    switch (m[1].toLowerCase()) {
      case "img":
      case "source":
        return inliner.rewriteAttr(tag, SRC_ATTR, docPath, "image");
      case "script":
        return inliner.rewriteAttr(tag, SRC_ATTR, docPath, "script");
      default: {
        const rel = tag.match(REL_ATTR);
        const relValue = (rel?.[3] ?? rel?.[4] ?? "").toLowerCase();
        if (/\bstylesheet\b/.test(relValue)) {
          return inliner.rewriteAttr(tag, HREF_ATTR, docPath, "style");
        }
        if (/\bicon\b/.test(relValue)) {
          return inliner.rewriteAttr(tag, HREF_ATTR, docPath, "image");
        }
        return tag;
      }
    }
  });
}

/**
 * Fetch the document at `entryPath` and everything it references relatively,
 * returning one `srcDoc` string.
 *
 * `urlFor` maps a root-relative path to the URL that serves it; `null` means
 * "this store has no such file" (a single-file artifact has no directory), and
 * the ref is left as-is. The entry document failing is an error — there is
 * nothing to show — while a failed subresource only keeps its original ref.
 */
export async function fetchSandboxDoc(
  entryPath: string,
  urlFor: (path: string) => string | null,
  fetchImpl: typeof fetch = fetch,
): Promise<string> {
  const entryUrl = urlFor(entryPath);
  if (!entryUrl) throw new Error(`no URL for ${entryPath}`);
  const res = await fetchImpl(entryUrl);
  if (!res.ok) throw new Error(`${entryPath} → HTTP ${res.status}`);
  const html = await res.text();
  return inlineDocAssets(html, entryPath, async (path) => {
    const url = urlFor(path);
    if (!url) throw new Error(`no URL for ${path}`);
    const r = await fetchImpl(url);
    if (!r.ok) throw new Error(`${path} → HTTP ${r.status}`);
    return r.blob();
  });
}
