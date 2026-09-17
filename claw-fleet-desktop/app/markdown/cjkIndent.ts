import type { Plugin } from "unified";
import type { Root, Element, Text, ElementContent } from "hast";
import { visit } from "unist-util-visit";

/**
 * Chinese first-line indent. Chinese typography convention: body paragraphs indent 2 characters on the first line;
 * English paragraphs do not. CSS cannot select elements based on the language of paragraph content, so at render time
 * we check if the first meaningful character of each `<p>` is CJK: if so, tag it with `cjk-indent`, which App.css
 * completes with `text-indent: 2em` (`1em ≈ 1 full-width character`, so `2em` = 2 Chinese characters).
 * English paragraphs get no tag and remain unchanged.
 *
 * Applies only to body paragraphs: list items / blockquotes / table cells skip it — they have their own
 * indentation context, and stacking first-line indent on top looks wrong. Must run after `rehype-sanitize`,
 * or the injected `className` gets sanitized away.
 */

// Hanzi (including extension A, CJK compatibility ideographs), CJK punctuation, full-width characters,
// plus the curved quotes common in Chinese (‘–‟ to cover “” ‘’ at paragraph start).
// We check the paragraph’s first meaningful character, so paragraphs opening with Chinese quotes indent correctly.
const CJK_LEADING =
  /[‘-‟　-〿㐀-䶿一-鿿豈-﫿＀-￯]/;

// In hast generated from markdown, body `<p>` parents are these containers
// (loose list li > p, blockquote > p, table td/th > p). Check the immediate parent
// to skip paragraphs inside them — they have their own indentation context, and adding first-line indent looks wrong.
const SKIP_PARENTS = new Set(["li", "blockquote", "td", "th"]);

/** Return the first non-whitespace character in the node's subtree; null if all-whitespace. */
function firstMeaningfulChar(node: ElementContent): string | null {
  if (node.type === "text") {
    const m = (node as Text).value.match(/\S/);
    return m ? m[0] : null;
  }
  if (node.type === "element") {
    for (const child of (node as Element).children) {
      const c = firstMeaningfulChar(child);
      if (c) return c;
    }
  }
  return null;
}

export const rehypeCjkIndent: Plugin<[], Root> = () => (tree) => {
  visit(tree, "element", (node: Element, _index, parent) => {
    if (node.tagName !== "p") return;
    if (
      parent &&
      parent.type === "element" &&
      SKIP_PARENTS.has((parent as Element).tagName)
    )
      return;
    const ch = firstMeaningfulChar(node);
    if (!ch || !CJK_LEADING.test(ch)) return;
    node.properties ??= {};
    const cls = node.properties.className;
    const list = Array.isArray(cls)
      ? cls.map(String)
      : cls != null
        ? [String(cls)]
        : [];
    if (!list.includes("cjk-indent")) list.push("cjk-indent");
    node.properties.className = list;
  });
};
