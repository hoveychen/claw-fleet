import type { Plugin } from "unified";
import type { Root, Element, Text, ElementContent } from "hast";
import { visit } from "unist-util-visit";

/**
 * CJK first-line indent. Typography convention: CJK body paragraphs indent 2
 * characters, English paragraphs don't. CSS can't select by content language, so at
 * render time check if each `<p>`'s first meaningful char is CJK: if so, tag with
 * `cjk-indent` class; `index.css` handles indent with `text-indent: 2em` (`1em ≈ 1
 * full-width char`, `2em` = 2 CJK characters). English paragraphs stay untagged.
 *
 * Applies only to body paragraphs: list items / blockquotes / table cells skip because
 * they have their own indent context; stacking first-line indent looks wrong. Must run
 * after `rehype-sanitize`, else injected `className` gets stripped.
 *
 * Sync with desktop `claw-fleet-desktop/app/markdown/cjkIndent.ts` (two apps are
 * separate vite packages; plugin logic copied, not shared).
 */

// CJK characters (including CJK Extension A, compatibility ideographs), CJK punct,
// full-width chars, plus curved quotes common in Chinese (‘–‟ covers “ “ ‘ ‘ opening
// Chinese quote passages). Match the first meaningful character; paragraphs opening with
// Chinese quotes also indent correctly.
const CJK_LEADING =
  /[‘-‟　-〿㐀-䶿一-鿿豈-﫿＀-￯]/;

// In markdown-generated hast, body `<p>` parents are these containers (loose list li > p,
// blockquote > p, table td/th > p), so checking direct parent skips their inner
// paragraphs — they have their own indent context, stacking first-line indent looks wrong.
const SKIP_PARENTS = new Set(["li", "blockquote", "td", "th"]);

/** Return first non-whitespace char in node subtree; null if all whitespace. */
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
