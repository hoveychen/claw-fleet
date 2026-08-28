// @vitest-environment jsdom
import { describe, expect, it } from "vitest";

import { fixSymbolBullets } from "./officeRender";

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
