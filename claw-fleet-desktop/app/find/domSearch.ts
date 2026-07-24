/**
 * Pure text search over a DOM subtree — the engine behind the in-app Cmd+F find
 * bar's main-document matches.
 *
 * It returns per-text-node match offsets (never mutates the DOM), so the caller
 * can wrap them in `Range`s and paint them with the CSS Custom Highlight API.
 * Keeping this free of `CSS.highlights` / layout calls is what lets it run under
 * jsdom in a unit test: give it a tree and a query, get back matches in document
 * order.
 *
 * A match is always contained within a single text node. That is the normal case
 * for a find query (the searched text rarely straddles an element boundary), and
 * it keeps the ranges trivially valid — a query split across `foo<mark>bar</mark>`
 * simply won't match "foobar", which is an acceptable limitation for find-in-page.
 */

/** One match: a slice `[start, end)` of a single text node's data. */
export interface FindMatch {
  node: Text;
  start: number;
  end: number;
}

/**
 * Collect every occurrence of `query` (case-insensitive) in the text nodes under
 * `root`, in document order.
 *
 * `shouldSkipElement`, when provided, prunes an entire subtree: any element it
 * returns `true` for is not descended into (used to skip the find bar itself,
 * `<script>`/`<style>`, and hidden content). Returns `[]` for an empty query.
 */
export function findMatches(
  root: Node,
  query: string,
  shouldSkipElement?: (el: Element) => boolean,
): FindMatch[] {
  const needle = query.toLowerCase();
  if (!needle) return [];

  const doc = root.ownerDocument ?? (root as Document);
  const walker = doc.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
    acceptNode(node: Node): number {
      if (node.nodeType === Node.ELEMENT_NODE) {
        return shouldSkipElement && shouldSkipElement(node as Element)
          ? NodeFilter.FILTER_REJECT // prune subtree
          : NodeFilter.FILTER_SKIP; // visit children, not the element itself
      }
      return NodeFilter.FILTER_ACCEPT; // a text node
    },
  });

  const matches: FindMatch[] = [];
  let current = walker.nextNode();
  while (current) {
    const text = current as Text;
    const hay = text.data.toLowerCase();
    let from = 0;
    for (;;) {
      const idx = hay.indexOf(needle, from);
      if (idx === -1) break;
      matches.push({ node: text, start: idx, end: idx + needle.length });
      from = idx + needle.length; // non-overlapping, matching browser find
    }
    current = walker.nextNode();
  }
  return matches;
}
