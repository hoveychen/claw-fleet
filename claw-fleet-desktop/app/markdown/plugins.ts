// The one markdown plugin chain every surface renders through — chat messages,
// decision cards, wiki docs, report cards. Kept free of Tauri APIs and CSS
// modules so plugins.test.ts can drive it directly.
import type { PluggableList } from "unified";
import remarkGfm from "remark-gfm";
import remarkCjkFriendly from "remark-cjk-friendly";
import remarkMath from "remark-math";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeKatex from "rehype-katex";
import { remarkCjkAutolinkFix } from "./cjkAutolinkFix";
import "katex/dist/katex.min.css";

/**
 * What `rehype-sanitize` lets through beyond GitHub's default schema.
 *
 * The default schema is built for prose, so two things it drops would break
 * features we do want:
 *
 * `remark-math` marks formulas as `<span class="math math-inline">`, and the
 * default schema allows no attributes on `span` at all. Strip that class and
 * `rehype-katex` finds nothing left to typeset — the formula renders as bare
 * text. KaTeX itself runs *after* sanitize, so its own (large, MathML-heavy)
 * output is never subject to this schema; only the marker class has to survive
 * the pass.
 *
 * Nothing else needs widening: GFM's table alignment rides on the `align`
 * attribute (already in the default schema's global allow-list) and task-list
 * checkboxes on `input[type=checkbox][disabled]`, both verified in
 * plugins.test.ts.
 */
const schema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    span: [
      ...(defaultSchema.attributes?.span ?? []),
      ["className", "math", "math-inline", "math-display"],
    ],
  },
};

/**
 * `remark-cjk-friendly` is load-bearing, not a nicety: CommonMark refuses to
 * open emphasis when the `**` sits between a CJK character and punctuation, so
 * `一个是**“引号开头”的加粗**` renders the asterisks literally (verified against
 * this exact chain). The fix has to live in the tokenizer — by the time a mdast
 * plugin could see the tree, the `**` is already plain text.
 */
export const safeRemarkPlugins: PluggableList = [
  remarkGfm,
  remarkCjkFriendly,
  remarkMath,
  remarkCjkAutolinkFix,
];

/**
 * Order matters. `rehype-raw` turns the model's raw HTML into real nodes,
 * `rehype-sanitize` then scrubs it, and only then does `rehype-katex` run — so
 * the untrusted HTML is sanitized while KaTeX's trusted output is not mangled
 * by the pass that would otherwise strip its markup.
 */
export const safeRehypePlugins: PluggableList = [
  rehypeRaw,
  [rehypeSanitize, schema],
  rehypeKatex,
];
