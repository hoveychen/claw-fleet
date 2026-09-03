// @vitest-environment jsdom
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { fixSymbolBullets, renderMarkdownInto, renderPptxInto } from "./officeRender";

/**
 * A 19-slide deck that renders in PowerPoint and rendered as a black box in the
 * app, cut down to its first slide.
 *
 * Its `[Content_Types].xml` declares nineteen `slideMaster` parts —
 * `slideMaster1.xml` through `slideMaster19.xml` — while the package contains
 * only `slideMaster1.xml`. PowerPoint ignores an override for a part that isn't
 * there; pptx-preview walks the override list and calls `.async()` on the
 * missing zip entry, which aborts the rest of the load. The layouts and slides
 * are read *after* the masters, so the deck arrives at the renderer with zero
 * slides and `slides[0].background` throws
 * "Cannot read properties of undefined (reading 'background')" — the message
 * the artifact stage put on screen.
 *
 * Verified by isolation against the real 396 KB deck: deleting only the
 * eighteen phantom overrides, changing nothing else, took it from
 * `layouts: 0, slides: 0` to `layouts: 1, slides: 19`.
 */
function phantomOverrideDeck(): Blob {
  // Vitest runs with the package root as cwd, so the path is relative to it
  // rather than to this file; a wrong cwd throws ENOENT rather than passing.
  const bytes = readFileSync("app/fixtures/phantom-overrides.pptx");
  return new Blob([new Uint8Array(bytes)]);
}

/**
 * Reproduces what docx-preview emits for a Word bulleted list, taken verbatim
 * from the rule observed in the running app:
 *
 *   p.docx-num-1001-0::before { content: "\9 "; font-family: Symbol; }
 *
 * U+F0B7 is the Symbol font's private-use slot for "•". Without the font — so,
 * on every Mac — the browser draws a tofu box instead.
 */
function hostWithBulletRule(): HTMLElement {
  const host = document.createElement("div");
  const style = document.createElement("style");
  style.textContent =
    'p.docx-num-1001-0::before { content: "\\9 "; font-family: Symbol; }';
  host.appendChild(style);
  // A stylesheet only exists once the element is in the document.
  document.body.appendChild(host);
  return host;
}

describe("fixSymbolBullets", () => {
  it("rewrites the private-use bullet Word wrote into a real one", () => {
    const host = hostWithBulletRule();

    fixSymbolBullets(host);

    const rule = [...(host.querySelector("style")!.sheet!.cssRules)][0] as CSSStyleRule;
    expect(rule.style.content).not.toMatch(/[-]/);
    expect(rule.style.content).toContain("•");
    // The symbol font has to go too: it is what would have been asked to draw
    // the replacement, and it is the font that isn't there.
    expect(rule.style.fontFamily.toLowerCase()).not.toContain("symbol");
  });

  it("leaves ordinary rules alone", () => {
    const host = document.createElement("div");
    const style = document.createElement("style");
    style.textContent = 'p.plain::before { content: "1. "; font-family: Aptos; }';
    host.appendChild(style);
    document.body.appendChild(host);

    fixSymbolBullets(host);

    const rule = [...(host.querySelector("style")!.sheet!.cssRules)][0] as CSSStyleRule;
    expect(rule.style.content).toContain("1. ");
    expect(rule.style.fontFamily).toContain("Aptos");
  });
});

describe("fixSymbolBullets across multiple rules", () => {
  it("fixes every bulleted list in the document, not just the first", () => {
    // A document with two lists produces two ::before rules, and every one of
    // them has to be rewritten — a half-fixed document still shows boxes.
    const host = document.createElement("div");
    const style = document.createElement("style");
    // "\uF0B7" is a TS escape, so what lands in the CSS text is the real
    // private-use character — the same byte the library emits.
    const rule = (cls: string) =>
      `p.${cls}::before { content: "\uF0B7\\9 "; font-family: Symbol; }`;
    style.textContent = rule("a") + rule("b");
    host.appendChild(style);
    document.body.appendChild(host);

    fixSymbolBullets(host);

    const rules = [...host.querySelector("style")!.sheet!.cssRules] as CSSStyleRule[];
    expect(rules).toHaveLength(2);
    for (const r of rules) expect(r.style.content).toContain("\u2022");
  });
});

describe("renderPptxInto — a deck whose content types over-declare", () => {
  it("renders the first slide instead of throwing", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);

    await renderPptxInto(phantomOverrideDeck(), host, 820);

    // The library builds one `.slide-wrapper` per rendered slide; before the
    // repair there was none, because the deck parsed to zero slides.
    expect(host.querySelector(".slide-wrapper")).not.toBeNull();
    expect(host.textContent).toContain("短歌行");
  });
});

describe("renderMarkdownInto", () => {
  it("renders markdown as html rather than dropping the source in", async () => {
    const host = document.createElement("div");

    await renderMarkdownInto("# 部署备料清单\n\n- 第一项\n- 第二项\n", host);

    expect(host.querySelector("h1")?.textContent).toBe("部署备料清单");
    expect(host.querySelectorAll("li")).toHaveLength(2);
    // The failure this replaces: the card showed the literal "#".
    expect(host.textContent).not.toContain("#");
  });

  it("keeps the CJK-friendly emphasis the stage gets", async () => {
    // Plain CommonMark refuses to open emphasis when ** sits between a CJK
    // character and punctuation, so a thumbnail on its own chain would show
    // raw asterisks where the stage shows bold. Same plugin list, same result.
    const host = document.createElement("div");

    await renderMarkdownInto("一个是**“口径每年变”这个特征**。所有 SaaS", host);

    expect(host.querySelector("strong")?.textContent).toBe("“口径每年变”这个特征");
    expect(host.textContent).not.toContain("**");
  });

  /**
   * An artifact is an agent-produced file and this is the one path that puts
   * its markup into `innerHTML` on the app's own origin — the stage's html mode
   * gets a cross-origin sandboxed frame instead. So the sanitize pass in the
   * shared chain is load-bearing, not tidiness.
   */
  it("scrubs script and event handlers out of embedded html", async () => {
    const host = document.createElement("div");

    await renderMarkdownInto(
      'ok\n\n<script>window.pwned = 1</script>\n\n<img src="x" onerror="window.pwned = 2">\n',
      host,
    );

    expect(host.querySelector("script")).toBeNull();
    expect(host.innerHTML).not.toContain("onerror");
  });
});

describe("renderMarkdownInto — remote images", () => {
  /**
   * A card renders as soon as it scrolls into view, so a deliverable carrying a
   * `<img src="https://…">` would make the artifacts page fan out requests to
   * whatever hosts its documents happen to reference, just from scrolling. The
   * stage does the same on an explicit open, which is a different bargain.
   * Nothing remote may reach the document — that is what the assertion checks,
   * because an src that never lands cannot be fetched.
   */
  it("replaces http(s) images with a placeholder instead of loading them", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);

    await renderMarkdownInto(
      "![release badge](https://img.shields.io/badge/release-v2.4.2-blue)\n",
      host,
    );

    expect(host.querySelector("img")).toBeNull();
    expect(host.innerHTML).not.toContain("img.shields.io");
    const ph = host.querySelector("[data-remote-image]");
    expect(ph?.textContent).toBe("release badge");
  });

  it("covers protocol-relative and uppercase-scheme urls too", async () => {
    const host = document.createElement("div");

    await renderMarkdownInto(
      '<img src="//evil.example/px.gif" alt="a">\n\n<img src="HTTPS://evil.example/b.png" alt="b">\n',
      host,
    );

    expect(host.querySelector("img")).toBeNull();
    expect(host.innerHTML).not.toContain("evil.example");
  });

  /**
   * A relative path can never resolve: `fleet artifact add` stores one file as
   * `artifacts/<id>/<name>`, so the document has no siblings. Left alone it
   * would fire a request that 404s and leave a broken-image glyph in the well —
   * the four `docs/…` refs in this repo's own README did exactly that. Only a
   * `data:` URI carries its own bytes, so only it survives.
   */
  it("placeholders relative images too, and keeps data: URIs", async () => {
    const host = document.createElement("div");

    await renderMarkdownInto(
      "![chart](docs/chart.png)\n\n![inline](data:image/gif;base64,R0lGODlhAQABAAAAACw=)\n",
      host,
    );

    const srcs = [...host.querySelectorAll("img")].map((i) => i.getAttribute("src"));
    expect(srcs).toEqual(["data:image/gif;base64,R0lGODlhAQABAAAAACw="]);
    expect(host.querySelector("[data-remote-image]")?.textContent).toBe("chart");
  });
});
