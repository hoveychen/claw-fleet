// @vitest-environment jsdom
import { describe, expect, it } from "vitest";

import { fixSymbolBullets, renderMarkdownInto } from "./officeRender";

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
