// Wiki knowledge-base client for the mobile web app. Docs are published on the
// desktop into ~/.fleet/wiki and read over the relay via `wiki_list` /
// `wiki_file` (see claw-fleet-core/src/mobile_relay.rs). Markdown renders inline
// with react-markdown; HTML/htmlDir docs render full-fidelity in a sandboxed
// iframe — every relative asset reference (img/css/js/font) is fetched over the
// relay and rewritten to a blob: URL so the doc looks the same as on desktop.

import { ASSET_REQUEST_TIMEOUT_MS, type FleetTransport } from "./transport";
import type { WikiDoc, WikiExportPayload, WikiFilePayload, WikiSearchHit } from "./types";

export type {
  WikiDoc,
  WikiVersion,
  WikiFilePayload,
  WikiSearchHit,
  WikiExportPayload,
} from "./types";

// ── Pure path / reference helpers (unit-tested in wiki.test.ts) ───────────────

/** Refs that point outside the doc — never rewritten to a blob. */
export function isExternalRef(ref: string): boolean {
  const r = ref.trim();
  return (
    r === "" ||
    r.startsWith("#") ||
    r.startsWith("//") ||
    /^[a-z][a-z0-9+.-]*:/i.test(r) // http:, https:, data:, blob:, mailto:, …
  );
}

/** Directory portion of a version-relative path ("a/b.html" → "a", "x" → ""). */
export function dirOf(relpath: string): string {
  const i = relpath.lastIndexOf("/");
  return i < 0 ? "" : relpath.slice(0, i);
}

/** Resolve `ref` (relative to `baseDir`, or version-root when it starts with
 *  "/") into a normalized version-relative path. Strips query/hash. Returns
 *  null when empty or when it escapes the version root via `..`. */
export function resolveRelPath(baseDir: string, ref: string): string | null {
  const clean = ref.split(/[?#]/, 1)[0].trim();
  if (clean === "") return null;
  const rooted = clean.startsWith("/");
  const segs = (rooted || !baseDir ? [] : baseDir.split("/")).concat(clean.split("/"));
  const out: string[] = [];
  for (const seg of segs) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (out.length === 0) return null; // would escape the version root
      out.pop();
    } else out.push(seg);
  }
  return out.length ? out.join("/") : null;
}

/** Text-based assets whose own contents reference further assets (recursed). */
export function looksTextual(mime: string): boolean {
  return /^text\/|(javascript|json|xml|svg)/i.test(mime);
}

type RefMap = (rawRef: string) => string | null;

/** Rewrite every relocatable asset reference in `text` using `map`. `map`
 *  returns the replacement (blob URL) or null to leave the ref untouched.
 *  Handles HTML `src`/`href`/`poster`, `srcset`, and CSS `url()` / `@import`. */
export function transformRefs(text: string, kind: "html" | "css", map: RefMap): string {
  const sub = (raw: string): string => {
    if (isExternalRef(raw)) return raw;
    return map(raw) ?? raw;
  };
  let out = text;
  if (kind === "html") {
    out = out.replace(
      /\b(src|href|poster)\s*=\s*(["'])(.*?)\2/gi,
      (_m, attr, q, ref) => `${attr}=${q}${sub(ref)}${q}`,
    );
    out = out.replace(/\bsrcset\s*=\s*(["'])(.*?)\1/gi, (_m, q, list) => {
      const rewritten = String(list)
        .split(",")
        .map((cand) => {
          const parts = cand.trim().split(/\s+/);
          if (parts[0]) parts[0] = sub(parts[0]);
          return parts.join(" ");
        })
        .join(", ");
      return `srcset=${q}${rewritten}${q}`;
    });
  }
  // CSS url()/@import appear in both <style> blocks/style attrs and .css files.
  out = out.replace(
    /url\(\s*(["']?)([^"')]+)\1\s*\)/gi,
    (_m, q, ref) => `url(${q}${sub(ref)}${q})`,
  );
  out = out.replace(
    /@import\s+(["'])(.*?)\1/gi,
    (_m, q, ref) => `@import ${q}${sub(ref)}${q}`,
  );
  return out;
}

/** Every relocatable ref in `text` (deduped) — used to know what to fetch. */
export function collectRefs(text: string, kind: "html" | "css"): string[] {
  const seen = new Set<string>();
  transformRefs(text, kind, (raw) => {
    seen.add(raw);
    return null;
  });
  return [...seen];
}

// ── Relay fetch + blob assembly (runtime) ─────────────────────────────────────

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

export async function listWikiDocs(client: FleetTransport): Promise<WikiDoc[]> {
  return client.request<WikiDoc[]>("wiki_list");
}

/** Full-text search over doc metadata + entry body. The relay short-circuits
 *  queries under 2 chars to an empty list, mirrored here so we don't round-trip
 *  a single keystroke. */
export async function searchWikiDocs(
  client: FleetTransport,
  query: string,
): Promise<WikiSearchHit[]> {
  if (query.trim().length < 2) return [];
  return client.request<WikiSearchHit[]>("wiki_search", { query });
}

/** Export one doc (single file for markdown/html, a zip for htmlDir) as bytes
 *  plus its suggested download filename. */
export async function exportWikiDoc(
  client: FleetTransport,
  slug: string,
  version: string,
): Promise<{ filename: string; mime: string; bytes: Uint8Array }> {
  const { filename, mime, base64 } = await client.request<WikiExportPayload>("wiki_export", {
    slug,
    version,
  });
  return { filename, mime, bytes: base64ToBytes(base64) };
}

/** Raw bytes + mime for one file (entry or asset) in a doc version. */
export async function fetchWikiFile(
  client: FleetTransport,
  slug: string,
  version: string,
  relpath: string,
): Promise<{ mime: string; bytes: Uint8Array }> {
  const { mime, base64 } = await client.request<WikiFilePayload>(
    "wiki_file",
    { slug, version, relpath },
    ASSET_REQUEST_TIMEOUT_MS,
  );
  return { mime, bytes: base64ToBytes(base64) };
}

/** Decode a file's bytes to text (markdown / html source). */
export async function fetchWikiText(
  client: FleetTransport,
  slug: string,
  version: string,
  relpath: string,
): Promise<string> {
  const { bytes } = await fetchWikiFile(client, slug, version, relpath);
  return new TextDecoder().decode(bytes);
}

export interface WikiHtmlBundle {
  /** Rewritten entry HTML to drop into an iframe `srcdoc`. */
  srcdoc: string;
  /** No-op — data: URIs need no cleanup. Kept for call-site stability. */
  revoke: () => void;
}

/** `data:` URI for a fetched asset. We deliberately use data: — NOT blob: —
 *  because the render iframe is fully sandboxed (`allow-scripts` only, no
 *  `allow-same-origin`, so a doc's own JS can't reach the parent's pairing
 *  secret). A sandboxed opaque-origin frame is blocked from loading a parent-
 *  origin blob: URL ("Not allowed to load local resource"), whereas data: URIs
 *  load fine. Charset params in the mime are stripped so the data: mediatype
 *  stays well-formed. */
function toDataUri(mime: string, bytes: Uint8Array): string {
  const mediatype = mime.split(";")[0].trim() || "application/octet-stream";
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return `data:${mediatype};base64,${btoa(bin)}`;
}

function textToDataUri(mime: string, text: string): string {
  return toDataUri(mime, new TextEncoder().encode(text));
}

/** Fetch an HTML/htmlDir doc's entry and every asset it (transitively) refers
 *  to, inlining assets as data: URIs and rewriting references so the whole
 *  thing renders offline-from-relay inside a sandboxed iframe. Recurses through
 *  text assets (css/js/svg) so `html → css → font/img` chains resolve; cycles
 *  break by leaving the ref as-is. */
export async function buildWikiHtml(
  client: FleetTransport,
  doc: WikiDoc,
  version: string,
): Promise<WikiHtmlBundle> {
  const cache = new Map<string, string>(); // relpath → data: URI
  const visiting = new Set<string>();

  // Returns a data: URI for a version-relative asset path, recursing into text
  // assets so their inner refs resolve first.
  const blobify = async (relpath: string): Promise<string | null> => {
    if (cache.has(relpath)) return cache.get(relpath)!;
    if (visiting.has(relpath)) return null; // cycle — leave the ref untouched
    visiting.add(relpath);
    try {
      const { mime, bytes } = await fetchWikiFile(client, doc.slug, version, relpath);
      let uri: string;
      if (looksTextual(mime)) {
        const kind = /html/i.test(mime) ? "html" : "css";
        const rewritten = await rewriteText(new TextDecoder().decode(bytes), relpath, kind);
        uri = textToDataUri(mime, rewritten);
      } else {
        uri = toDataUri(mime, bytes);
      }
      cache.set(relpath, uri);
      return uri;
    } catch {
      return null; // missing/oversize asset — leave the ref, don't abort the doc
    } finally {
      visiting.delete(relpath);
    }
  };

  // Rewrite one text file's refs, resolving each against that file's own dir.
  const rewriteText = async (
    text: string,
    ownRelpath: string,
    kind: "html" | "css",
  ): Promise<string> => {
    const baseDir = dirOf(ownRelpath);
    const urls = new Map<string, string>(); // rawRef → blob URL
    for (const raw of collectRefs(text, kind)) {
      const resolved = resolveRelPath(baseDir, raw);
      if (!resolved) continue;
      const url = await blobify(resolved);
      if (url) urls.set(raw, url);
    }
    return transformRefs(text, kind, (raw) => urls.get(raw) ?? null);
  };

  const entryText = await fetchWikiText(client, doc.slug, version, doc.entry);
  const srcdoc = await rewriteText(entryText, doc.entry, "html");
  return { srcdoc, revoke: () => {} };
}
