import { describe, expect, it } from "vitest";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkGfm from "remark-gfm";
import remarkRehype from "remark-rehype";
import rehypeStringify from "rehype-stringify";

import { chunkMarkdown } from "./chunkMarkdown";

const processor = unified()
  .use(remarkParse)
  .use(remarkGfm)
  .use(remarkRehype, { allowDangerousHtml: true })
  .use(rehypeStringify, { allowDangerousHtml: true });

function html(md: string): string {
  return String(processor.processSync(md));
}

/**
 * The real contract. Each chunk is parsed by its own processor and knows
 * nothing about its neighbours, so "safe to cut here" means exactly: rendering
 * the chunks separately and concatenating gives what rendering the whole
 * document gives.
 *
 * Two things are normalised away, and only two. Whitespace *between* tags,
 * because a chunk boundary eats the newline that would otherwise sit there and
 * `</pre><p>` renders exactly like `</pre> <p>`. And a bullet list split across
 * a boundary — `<ul>a</ul><ul>b</ul>` instead of `<ul>ab</ul>` — which renders
 * identically because every bullet is the same glyph, and forbidding it would
 * leave a document that is one long bullet list with no legal cut anywhere.
 *
 * Everything else is compared as-is, so a cut through a fence, a table, an
 * ordered list or a paragraph shows up here as a structural difference.
 */
function renderEquivalent(src: string, chunks: string[]): void {
  const structure = (s: string) =>
    s
      .replace(/<\/ul>\s*<ul>/g, "")
      .replace(/>\s+</g, "><")
      .replace(/\s+/g, " ")
      .trim();
  expect(structure(chunks.map(html).join(""))).toBe(structure(html(src)));
}

/** Repeat `unit` until the result is at least `bytes` long. */
function bulk(unit: string, bytes: number): string {
  let out = "";
  while (out.length < bytes) out += unit;
  return out;
}

describe("chunkMarkdown", () => {
  it("reproduces the source exactly when joined", () => {
    // This is a rendering strategy, never a content one: not one byte is
    // dropped, summarised or elided.
    const src =
      "# Title\n\n" +
      bulk("Some prose paragraph about the thing.\n\n", 20_000) +
      "```js\nconst x = 1;\n```\n\n" +
      bulk("- bullet item\n", 20_000) +
      "\nlast line without newline";
    const chunks = chunkMarkdown(src, 8 * 1024);
    expect(chunks.join("")).toBe(src);
    expect(chunks.length).toBeGreaterThan(1);
  });

  it("keeps a source that ends with a newline ending with one", () => {
    const src = bulk("para\n\n", 20_000);
    const chunks = chunkMarkdown(src, 4 * 1024);
    expect(chunks.join("")).toBe(src);
    // Trailing blank lines must not become a chunk of their own — the reader
    // would be asked to scroll for a screenful of nothing.
    expect(chunks.every((c) => c.trim() !== "")).toBe(true);
  });

  it("renders the same whether chunked or whole, across mixed structures", () => {
    const src =
      "# Title\n\n" +
      bulk("Prose paragraph that goes on.\n\n", 12_000) +
      "| a | b |\n| - | - |\n" +
      bulk("| 1 | 2 |\n", 8_000) +
      "\n" +
      bulk("- bullet\n", 8_000) +
      "\n" +
      Array.from({ length: 300 }, (_, i) => `${i + 1}. item ${i}\n`).join("") +
      "\n" +
      bulk("> quoted\n", 4_000) +
      "\ntail paragraph\n";
    const chunks = chunkMarkdown(src, 4 * 1024);
    expect(chunks.join("")).toBe(src);
    expect(chunks.length).toBeGreaterThan(1);
    renderEquivalent(src, chunks);
  });

  it("renders the same when a fenced block is larger than the target", () => {
    // Blank lines *inside* the fence are the trap: they look like legal cut
    // points to anything that isn't tracking the fence, and a cut there turns
    // the rest of the document into code.
    const src =
      "intro\n\n```\n" +
      bulk("code line\n\nmore code\n\n", 40_000) +
      "```\n\ntail paragraph\n";
    const chunks = chunkMarkdown(src, 4 * 1024);
    expect(chunks.join("")).toBe(src);
    renderEquivalent(src, chunks);
  });

  it("renders the same for a loose ordered list whose items are blank-separated", () => {
    // A loose list has blank lines between items, so it *does* offer syntactic
    // cut points — and cutting one restarts the numbering at 1 in the next
    // chunk. Only the ordered-list rule prevents that.
    const items = Array.from({ length: 1500 }, (_, i) => `${i + 1}. item ${i}\n\n`).join("");
    const src = "lead in\n\n" + items + "tail\n";
    const chunks = chunkMarkdown(src, 4 * 1024);
    expect(chunks.join("")).toBe(src);
    renderEquivalent(src, chunks);
  });

  it("renders the same for a blockquote longer than the target", () => {
    const src =
      "lead\n\n" + bulk("> quoted line\n>\n", 20_000) + "\ntail\n";
    const chunks = chunkMarkdown(src, 4 * 1024);
    expect(chunks.join("")).toBe(src);
    renderEquivalent(src, chunks);
  });

  it("renders the same for a loose blockquote whose paragraphs are blank-separated", () => {
    // Blank lines *between* quoted paragraphs are real cut points, and cutting
    // one turns a single blockquote into two stacked ones.
    const src = "lead\n\n" + bulk("> quoted paragraph\n\n", 20_000) + "tail\n";
    const chunks = chunkMarkdown(src, 4 * 1024);
    expect(chunks.join("")).toBe(src);
    renderEquivalent(src, chunks);
  });

  it("renders the same when paragraphs run across several lines", () => {
    // A multi-line paragraph has no blank line inside it, so cutting anywhere
    // but at a blank line would split one <p> into two.
    const src =
      "lead\n\n" +
      bulk("first line of a paragraph\nsecond line of it\nthird line of it\n\n", 20_000) +
      "tail\n";
    const chunks = chunkMarkdown(src, 4 * 1024);
    expect(chunks.join("")).toBe(src);
    renderEquivalent(src, chunks);
  });

  it("renders the same when a list item is continued by an indented block", () => {
    // The blank line before the indented continuation is preceded by a plain
    // bullet (safe-looking) and followed by an indent that belongs to it —
    // only looking at the line *after* the cut catches this.
    const src =
      "lead\n\n" +
      bulk("- bullet item\n\n      continued literal block\n\n", 20_000) +
      "tail\n";
    const chunks = chunkMarkdown(src, 4 * 1024);
    expect(chunks.join("")).toBe(src);
    renderEquivalent(src, chunks);
  });

  it("leaves a document that fits in one chunk alone", () => {
    const src = "# Small\n\nJust a paragraph.\n";
    expect(chunkMarkdown(src)).toEqual([src]);
  });

  it("returns nothing for an empty source", () => {
    expect(chunkMarkdown("")).toEqual([]);
  });

  it("returns one chunk when the document offers no legal cut", () => {
    const src = "| a |\n" + bulk("| 1 |\n", 30_000);
    expect(chunkMarkdown(src, 4 * 1024)).toEqual([src]);
  });
});
