// Mirrors the desktop chain (claw-fleet-desktop/app/markdown/plugins.ts) — the
// two apps are separate vite packages, so the list is duplicated rather than
// shared. Keep them in step: a message that bolds on the desktop must bold on
// the phone.
//
// "Keep them in step" on its own did not hold: this list silently drifted two
// plugins behind the desktop (`singleTilde: false` and `remarkCjkAutolinkFix`),
// and both gaps were user-visible on the phone for as long as they lasted —
// because the desktop had tests for them and this side had none. plugins.test.ts
// next to this file now pins both. Add a case there for anything you mirror.
import type { Plugin, PluggableList } from "unified";
import type { Root, Element } from "hast";
import { visit } from "unist-util-visit";
import remarkGfm from "remark-gfm";
import remarkBreaks from "remark-breaks";
import remarkCjkFriendly from "remark-cjk-friendly";
import remarkMath from "remark-math";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeKatex from "rehype-katex";
import { remarkCjkAutolinkFix } from "./cjkAutolinkFix";
import { rehypeCjkIndent } from "./cjkIndent";
import {
  EXPLAIN_MARK_CLASS,
  EXPLAIN_MARK_QUOTE_PROP,
  remarkExplainMarks,
} from "../../../shared-ts/explainMarks";
import { remarkCurrencyDollar } from "../../../shared-ts/currencyDollar";
import "katex/dist/katex.min.css";

/**
 * Two widenings, mirrored from the desktop chain — keep them in step.
 *
 * 1. `remark-math` tags formulas with `<span class="math …">` and the default
 *    schema allows no `span` attributes at all, so without this the class is
 *    stripped and `rehype-katex` has nothing left to typeset. KaTeX runs after
 *    sanitize, so its own output is never subject to this schema.
 *
 * 2. The default schema allows no SVG tags, so a model that answers "draw the
 *    circuit" with inline `<svg>` (which the chat brief invites) has every
 *    `<svg>/<rect>/<line>/…` stripped, leaving only `<text>` to collapse into a
 *    run-on paragraph. `SVG_TAGS`/`SVG_ATTRS` re-admit the static drawing
 *    primitives and their inert geometry/presentation attributes — deliberately
 *    not `script`/`foreignObject`/`a`/`image`/`animate*`, nor `href`/`on*`.
 *    Attribute names are hast property names (e.g. `stroke-width` →
 *    `strokeWidth`). Verified in the desktop plugins.test.ts.
 */
const SVG_TAGS = [
  "svg", "g", "defs", "title", "desc", "symbol", "use",
  "path", "rect", "circle", "ellipse", "line", "polyline", "polygon",
  "text", "tspan",
  "marker", "linearGradient", "radialGradient", "stop", "pattern", "clipPath",
];

const SVG_ATTRS = [
  "viewBox", "xmlns", "xmlnsXlink", "version", "preserveAspectRatio",
  "width", "height", "x", "y", "cx", "cy", "r", "rx", "ry",
  "x1", "y1", "x2", "y2", "d", "points", "transform", "gradientTransform",
  "className", "id", "role",
  "fill", "fillOpacity", "fillRule", "stroke", "strokeWidth", "strokeOpacity",
  "strokeLineCap", "strokeLineJoin", "strokeDashArray", "strokeDashOffset",
  "strokeMiterLimit", "opacity", "clipPath", "clipRule",
  "fontSize", "fontFamily", "fontWeight", "fontStyle", "textAnchor",
  "dominantBaseline", "letterSpacing",
  "offset", "stopColor", "stopOpacity", "gradientUnits", "spreadMethod",
  "patternUnits", "markerStart", "markerMid", "markerEnd",
  "markerWidth", "markerHeight", "refX", "refY", "orient",
];

const schema = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames ?? []), ...SVG_TAGS],
  // `<style>` is not admitted, but sanitize keeps a dropped element's children by
  // default, so an SVG's `<style>.t{font:12px …}</style>` leaked its rules as
  // visible text. Strip the content too, as the default schema does for `<script>`.
  // Mirrored from the desktop chain; pinned in plugins.test.ts.
  strip: [...(defaultSchema.strip ?? []), "style"],
  attributes: {
    ...defaultSchema.attributes,
    span: [
      ...(defaultSchema.attributes?.span ?? []),
      // `explain-mark` / `dataExplainQuote`: the `[?text]` annotation span
      // `remarkExplainMarks` (shared-ts/explainMarks.ts) emits — sanitize
      // scrubs every element, raw or not, so both have to be admitted here.
      // Pinned in plugins.test.ts.
      ["className", "math", "math-inline", "math-display", EXPLAIN_MARK_CLASS],
      EXPLAIN_MARK_QUOTE_PROP,
    ],
    "*": [...(defaultSchema.attributes?.["*"] ?? []), ...SVG_ATTRS],
  },
};

/** A block-level `<svg …>` open tag, quoted attribute values allowed to hold `>`. */
const SVG_OPEN_TAG = /^(\s*<svg\b(?:[^>"']|"[^"]*"|'[^']*')*>)(.*)$/;

/**
 * A bare `<svg>` opens a CommonMark *type-7* HTML block, and — unlike a
 * `<script>`/`<pre>`/`<style>` block — a type-7 block ends at the first blank
 * line. Models routinely separate an inline SVG's logical groups with blank
 * lines; that blank line silently truncates the drawing: every tag after it
 * escapes the `<svg>` and the browser renders it as an empty inline element, so
 * the diagram shows as a near-blank box (the "svg renders blank" report). Drop
 * blank lines that sit *inside* a top-level svg span so the whole drawing stays
 * one HTML block. Fenced code is left untouched, so an SVG shown as a code
 * sample keeps its original formatting.
 *
 * The type-7 start condition also requires the open tag to be *alone* on its
 * line. `<svg viewBox="…"><defs>` fails it, so the svg starts a paragraph of
 * inline HTML instead — and the first `<style>`/`<pre>` line, which may
 * interrupt a paragraph, ends it there: the drawing is cut off after `<defs>`
 * and the rest spills into the prose. So anything after a block-level `<svg …>`
 * open tag is moved to its own line.
 */
// Mirrored from claw-fleet-desktop/app/markdown/plugins.ts — every
// <ReactMarkdown> here runs its text through it (pinned in mdCoverage.test.ts).
export function normalizeSvgBlankLines(text: string): string {
  if (!text.includes("<svg")) return text;
  const lines = text.split("\n");
  const out: string[] = [];
  let fence: string | null = null; // active ``` / ~~~ fence marker, if any
  let depth = 0; // open-svg nesting level for the buffered span
  let buf: string[] = []; // lines held while inside an <svg> span

  // Emit the buffered span. Blank lines are dropped only when the span closed
  // cleanly (a balanced </svg>); an unbalanced span — e.g. prose that merely
  // mentions `<svg>` and never closes it — is emitted verbatim so ordinary
  // paragraph breaks after it survive.
  const flush = (stripBlanks: boolean) => {
    for (const l of buf) {
      if (stripBlanks && l.trim() === "") continue;
      out.push(l);
    }
    buf = [];
    depth = 0;
  };

  for (const line of lines) {
    const trimmed = line.trim();
    const fenceMatch = /^(```+|~~~+)/.exec(trimmed);
    if (fenceMatch) {
      // A code fence can't open inside a real inline SVG, so any span still
      // open here was unbalanced — emit it untouched before the fence.
      if (depth > 0) flush(false);
      if (fence && trimmed.startsWith(fence)) fence = null;
      else if (!fence) fence = fenceMatch[1];
      out.push(line);
      continue;
    }
    if (fence) {
      out.push(line);
      continue;
    }
    // Only a *block-level* `<svg>` (at line start, bar leading whitespace) opens
    // the HTML block that the blank-line truncation hits. A mid-line `<svg`
    // — a prose mention, usually inside `code` — is ignored, so it can't start a
    // span and swallow the paragraphs after it.
    if (depth === 0 && !/^\s*<svg\b/.test(line)) {
      out.push(line);
      continue;
    }
    const open = depth === 0 ? SVG_OPEN_TAG.exec(line) : null;
    if (open && open[2].trim() !== "") buf.push(open[1], open[2]);
    else buf.push(line);
    depth += (line.match(/<svg\b/g) ?? []).length;
    depth -= (line.match(/<\/svg\s*>/g) ?? []).length;
    if (depth <= 0) flush(true); // balanced close → safe to drop inner blanks
  }
  if (buf.length) flush(false); // reached EOF mid-span → unbalanced, keep blanks
  return out.join("\n");
}

/**
 * `marker-end="url(#arrow)"` → `marker-end="url(#user-content-arrow)"`.
 *
 * `rehype-sanitize` prefixes every `id` with `user-content-` (GitHub's clobber
 * guard), but the `url(#…)` references pointing at those ids keep the bare name,
 * so every arrowhead, gradient fill and clip-path in an inline SVG resolved to
 * nothing. Rewrite the references to the prefixed id. Must run after sanitize,
 * which is what adds the prefix. Mirrored from the desktop chain
 * (claw-fleet-desktop/app/markdown/plugins.ts); pinned in plugins.test.ts.
 */
export const rehypeSvgUrlRefs: Plugin<[], Root> = () => (tree) => {
  visit(tree, "element", (node: Element) => {
    const props = node.properties;
    if (!props) return;
    for (const [key, value] of Object.entries(props)) {
      if (typeof value !== "string" || !value.includes("url(")) continue;
      props[key] = value.replace(
        /url\(\s*(['"]?)#(?!user-content-)/g,
        (_m, q: string) => `url(${q}#user-content-`,
      );
    }
  });
};

/**
 * `remark-cjk-friendly` is what makes `一个是**“引号开头”的加粗**` bold at all:
 * CommonMark won't open emphasis when `**` sits between a CJK character and
 * punctuation, and the fix has to happen in the tokenizer.
 */
export const mdRemarkPlugins: PluggableList = [
  // A bare home path starts with `~`. With remark-gfm's permissive default,
  // two paths such as `~/.claude/skills` and `~/.codex/skills` can swallow
  // everything between them into a <del>. GFM's standard `~~text~~` form
  // remains enabled when the single-tilde extension is disabled.
  [remarkGfm, { singleTilde: false }],
  // A single `\n` (soft break) renders as a real line break, not a space — handoff
  // notes and chat messages often lean on bare newlines instead of blank lines.
  remarkBreaks,
  remarkCjkFriendly,
  remarkMath,
  // `$14.84 … $0.054` is two amounts, not a formula (shared-ts/currencyDollar.ts).
  remarkCurrencyDollar,
  // GFM's autolink literal doesn't stop at CJK, so `见 https://example.com，然后`
  // swallows the comma and everything after it into the href.
  remarkCjkAutolinkFix,
  // `[?text]` → `<span class="explain-mark">`, the agent's own "this may need
  // explaining" annotation (shared-ts/explainMarks.ts); mirrored from the
  // desktop chain. Clickability is the `span` component's call (markdown/
  // explainMarks.tsx). Pinned in explainMarks.test.ts.
  remarkExplainMarks,
];

/** raw → sanitize → katex: scrub the model's HTML, then emit KaTeX's trusted DOM. */
export const mdRehypePlugins: PluggableList = [
  rehypeRaw,
  [rehypeSanitize, schema],
  rehypeKatex,
  // After sanitize, which is what adds the id prefix it points refs at.
  rehypeSvgUrlRefs,
  // Runs last so the `cjk-indent` class it adds to CJK-leading <p> survives the
  // sanitize pass above (className is globally allow-listed by `schema`).
  rehypeCjkIndent,
];
