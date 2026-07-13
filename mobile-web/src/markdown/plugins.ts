// Mirrors the desktop chain (claw-fleet-desktop/app/markdown/plugins.ts) — the
// two apps are separate vite packages, so the list is duplicated rather than
// shared. Keep them in step: a message that bolds on the desktop must bold on
// the phone.
import type { PluggableList } from "unified";
import remarkGfm from "remark-gfm";
import remarkCjkFriendly from "remark-cjk-friendly";
import remarkMath from "remark-math";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeKatex from "rehype-katex";
import "katex/dist/katex.min.css";

/**
 * Only widening: `remark-math` tags formulas with `<span class="math …">` and
 * the default schema allows no `span` attributes at all, so without this the
 * class is stripped and `rehype-katex` has nothing left to typeset. KaTeX runs
 * after sanitize, so its own output is never subject to this schema.
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
 * `remark-cjk-friendly` is what makes `一个是**“引号开头”的加粗**` bold at all:
 * CommonMark won't open emphasis when `**` sits between a CJK character and
 * punctuation, and the fix has to happen in the tokenizer.
 */
export const mdRemarkPlugins: PluggableList = [
  remarkGfm,
  remarkCjkFriendly,
  remarkMath,
];

/** raw → sanitize → katex: scrub the model's HTML, then emit KaTeX's trusted DOM. */
export const mdRehypePlugins: PluggableList = [
  rehypeRaw,
  [rehypeSanitize, schema],
  rehypeKatex,
];
