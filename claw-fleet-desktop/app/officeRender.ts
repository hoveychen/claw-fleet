/**
 * The three Office renderers behind one narrow interface, so the full-size
 * stage (`OfficePreview`) and the grid thumbnail (`ArtifactThumb`) call the
 * same code instead of each keeping its own copy of "which library, which
 * options, which quirks".
 *
 * Every function here does its own `await import()`, which is what keeps the
 * ~1.6 MB of renderers out of the main bundle: a caller that only ever opens
 * a .docx never pulls the pptx chunk. Importing this module is cheap — it is
 * the libraries that are not.
 */

/** 16:9 — the modern deck default. pptx-preview wants explicit pixels. */
export const SLIDE_RATIO = 9 / 16;

export type CellValue = string | number | boolean | Date | null;
export interface ParsedSheet {
  sheet: string;
  data: CellValue[][];
}

export async function fetchBlob(url: string): Promise<Blob> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return await res.blob();
}

/**
 * Render a .docx into `host` as real page boxes.
 *
 * `inWrapper` is what produces those pages; without it the document becomes one
 * long unstyled flow, which is the thing a .docx viewer exists to avoid.
 */
export async function renderDocxInto(blob: Blob, host: HTMLElement): Promise<void> {
  const { renderAsync } = await import("docx-preview");
  await renderAsync(blob, host, undefined, {
    inWrapper: true,
    breakPages: true,
    ignoreLastRenderedPageBreak: false,
  });
  fixSymbolBullets(host);
}

/**
 * Render a deck into `host` at `width` px, showing its first slide.
 *
 * Always "slide" mode, never "list": in list mode the library lays every slide
 * at the same offset, so a two-page deck paints both pages on top of each
 * other (verified against a real pandoc-produced .pptx). It also paints its
 * pagination before the first slide exists, so a freshly opened deck reads
 * "0/2" until clicked — hence the explicit restate.
 */
export async function renderPptxInto(
  blob: Blob,
  host: HTMLElement,
  width: number,
): Promise<void> {
  const { init } = await import("pptx-preview");
  const previewer = init(host, {
    width,
    height: Math.round(width * SLIDE_RATIO),
    mode: "slide",
  });
  await previewer.preview(await blob.arrayBuffer());
  previewer.updatePagination();
}

/**
 * Parse every sheet of an .xlsx to values.
 *
 * `read-excel-file/browser` rather than the package root: it ships one build
 * per environment and has no root export at all. Deliberately not SheetJS —
 * the copy on npm is frozen at 0.18.5 with two known CVEs and the maintained
 * build lives outside the registry.
 */
export async function readSheets(blob: Blob): Promise<ParsedSheet[]> {
  const readXlsxFile = (await import("read-excel-file/browser")).default;
  return (await readXlsxFile(blob)) as ParsedSheet[];
}

/**
 * Render markdown into `host` as sanitized HTML.
 *
 * Deliberately the string pipeline rather than the `TextBlock` component the
 * stage uses. `ArtifactThumb` is imperative DOM that caches `host.innerHTML`
 * once the render settles, and TextBlock's Prism / KaTeX / Mermaid passes are
 * asynchronous — reading innerHTML after mounting it would cache a half-painted
 * card. None of those three passes is worth anything at 0.28 scale anyway. What
 * *is* shared is the plugin chain (`safeRemarkPlugins` / `safeRehypePlugins`),
 * so a heading, a table and a CJK bold run come out looking like the stage's.
 *
 * That chain includes `rehype-sanitize`, which is load-bearing here and not
 * merely tidy: an artifact is an agent-produced file, and this is the one path
 * in the app that puts its markup into `innerHTML` on the main origin.
 */
export async function renderMarkdownInto(text: string, host: HTMLElement): Promise<void> {
  const [{ unified }, remarkParse, remarkRehype, rehypeStringify, plugins] = await Promise.all([
    import("unified"),
    import("remark-parse"),
    import("remark-rehype"),
    import("rehype-stringify"),
    import("./markdown/plugins"),
  ]);
  const html = String(
    unified()
      .use(remarkParse.default)
      .use(plugins.safeRemarkPlugins)
      // What react-markdown does internally: rehype-raw (first in the safe
      // chain) needs the raw html nodes to still be in the tree when it runs.
      .use(remarkRehype.default, { allowDangerousHtml: true })
      .use(plugins.safeRehypePlugins)
      .use(rehypeStringify.default, { allowDangerousHtml: true })
      .processSync(text),
  );
  // Via DOMParser rather than straight into `host.innerHTML`: the parsed
  // document has no browsing context, so nothing in it fetches. Assigning the
  // string first and scrubbing after would be too late — the browser starts
  // loading an <img> the moment it lands in a live document, which is the
  // request this is here to prevent.
  const parsed = new DOMParser().parseFromString(html, "text/html");
  defuseRemoteImages(parsed);
  host.innerHTML = parsed.body.innerHTML;
}

/**
 * Replace every `<img>` a card cannot honestly draw with an inert placeholder.
 * Only `data:` URIs survive, because only they carry their own bytes.
 *
 * Two separate reasons converge on the same rule:
 *
 *   - **Remote** (`https://…`, or protocol-relative `//host/x`) must not be
 *     fetched. A card renders as soon as it scrolls into view, so without this,
 *     opening the artifacts page fans out requests to whatever hosts the stored
 *     documents happen to reference — a README's shields.io badges were the
 *     case that made this visible. The stage does load them, but only when
 *     someone opens that one document on purpose; a grid that reaches out on
 *     scroll is a different bargain, and not one the user agreed to.
 *   - **Relative** (`docs/hero.png`) *cannot* resolve. An artifact is a single
 *     file — `fleet artifact add` takes one path and the store lays it down as
 *     `artifacts/<id>/<name>` — so the document has no siblings to point at.
 *     The request goes out, 404s, and the well shows a broken-image glyph.
 *
 * An empty src lands here too: `rehype-sanitize` drops a src it dislikes (an
 * uppercase `HTTPS://` scheme is one), and what it leaves is a srcless `<img>`
 * — no request, but the same broken glyph.
 */
function defuseRemoteImages(doc: Document): void {
  for (const img of Array.from(doc.querySelectorAll("img"))) {
    const src = img.getAttribute("src") ?? "";
    if (/^data:/i.test(src)) continue;
    const box = doc.createElement("span");
    box.setAttribute("data-remote-image", "");
    box.textContent = img.getAttribute("alt") || "";
    img.replaceWith(box);
  }
}

/** Cells arrive as parsed values; a Date must not render as its ISO string. */
export function formatCell(cell: CellValue): string {
  if (cell === null || cell === undefined) return "";
  if (cell instanceof Date) return cell.toLocaleDateString();
  return String(cell);
}

/**
 * Replace the private-use bullet code points Word documents carry.
 *
 * Word writes list markers as a character from a symbol font's private use
 * area — a "•" is U+F0B7 in `Symbol`, and Wingdings does the same at other
 * offsets — and docx-preview faithfully reproduces that as
 * `content: "\9 "; font-family: Symbol`. On any machine without those
 * fonts installed (every Mac, most Linux) the browser has nothing to draw and
 * every bullet renders as a tofu box. Verified in the real web build against a
 * pandoc-produced .docx: the CSS rule really is `p.docx-num-1001-0::before`
 * with U+F0B7 inside it.
 *
 * So after rendering, walk the stylesheets the library injected and rewrite
 * those code points to their ordinary Unicode equivalents. Unmapped private-use
 * characters fall back to "•": a list marker is decoration, and the wrong
 * bullet is strictly better than a box.
 */
export function fixSymbolBullets(host: HTMLElement): void {
  for (const styleEl of host.querySelectorAll("style")) {
    const sheet = styleEl.sheet;
    if (!sheet) continue;
    for (const rule of Array.from(sheet.cssRules)) {
      const style = (rule as CSSStyleRule).style as CSSStyleDeclaration | undefined;
      const content = style?.content;
      if (!style || !content) continue;
      // One pass, then compare — rather than testing for a private-use
      // character and replacing it in a second scan.
      const replaced = content.replace(PRIVATE_USE, BULLET);
      if (replaced === content) continue;
      style.content = replaced;
      // The symbol font has to go with it: it is the font that isn't there, and
      // leaving it would ask the browser to draw the *replacement* out of it.
      style.fontFamily = "inherit";
    }
  }
}

/** The Unicode private use area, where every symbol-font glyph Word emits lands. */
const PRIVATE_USE = /[\ue000-\uf8ff]/g;

/**
 * Every private-use marker becomes the same bullet, deliberately.
 *
 * A symbol font puts its glyph at `0xF000 + the byte`, so a faithful table
 * would need one entry per marker Word can emit — a checkmark here, a diamond
 * there — and only U+F0B7 ("\u2022" in `Symbol`) has actually been observed in
 * a real document. The rest would be written from memory, and a
 * plausible-but-wrong glyph is worse than an honest one: this is a list marker,
 * and any marker beats a box.
 */
const BULLET = "\u2022";
