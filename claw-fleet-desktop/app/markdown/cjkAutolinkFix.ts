import type { Plugin } from "unified";
import type { Root, Link, Text, PhrasingContent } from "mdast";
import { visit, SKIP } from "unist-util-visit";

// CJK punctuation + ideographs that micromark-extension-gfm-autolink-literal
// fails to treat as URL terminators. Detected here so we can crop them (and
// anything past them) out of autolinked URLs.
const CJK_BOUNDARY =
  /[　-〿︰-﹏＀-￯一-鿿㐀-䶿぀-ヿ가-힯]/;

export const remarkCjkAutolinkFix: Plugin<[], Root> = () => (tree) => {
  visit(tree, "link", (node: Link, index, parent) => {
    if (!parent || index == null) return;
    if (node.children.length !== 1) return;
    const child = node.children[0];
    if (child.type !== "text") return;
    if (child.value !== node.url) return;

    const match = child.value.match(CJK_BOUNDARY);
    if (!match || match.index == null || match.index === 0) return;

    const head = child.value.slice(0, match.index);
    const tail = child.value.slice(match.index);

    const fixedLink: Link = {
      type: "link",
      url: head,
      title: node.title,
      children: [{ type: "text", value: head }],
    };
    const tailNode: Text = { type: "text", value: tail };
    const siblings = parent.children as PhrasingContent[];
    siblings.splice(index, 1, fixedLink, tailNode);
    return [SKIP, index + 2];
  });
};
