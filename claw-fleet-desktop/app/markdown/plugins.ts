// The one markdown plugin chain every surface renders through — chat messages,
// decision cards, wiki docs, report cards. Kept free of Tauri APIs and CSS
// modules so plugins.test.ts can drive it directly.
import type { PluggableList } from "unified";
import remarkGfm from "remark-gfm";
import remarkBreaks from "remark-breaks";
import remarkCjkFriendly from "remark-cjk-friendly";
import remarkMath from "remark-math";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeKatex from "rehype-katex";
import { remarkCjkAutolinkFix } from "./cjkAutolinkFix";
import { rehypeCjkIndent } from "./cjkIndent";
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
 * GFM's table alignment rides on the `align` attribute (already in the default
 * schema's global allow-list) and task-list checkboxes on
 * `input[type=checkbox][disabled]`, both verified in plugins.test.ts.
 *
 * The default schema also allows *no* SVG tags, so a model that answers "draw
 * the circuit" with inline `<svg>` — which the chat brief explicitly invites —
 * has every `<svg>/<rect>/<line>/…` stripped, leaving only the `<text>` content
 * to collapse into a run-on paragraph. `SVG_TAGS` re-admits the static drawing
 * primitives (deliberately *not* `script`, `foreignObject`, `a`, `image`, or
 * the `animate*` family — the tags that turn SVG into a script/navigation
 * vector), and `SVG_ATTRS` re-admits their inert geometry/presentation
 * attributes (no `href`/`xlink:href`, no `on*`). Attribute names are the hast
 * property names sanitize matches on (camelCase for hyphenated SVG attrs, e.g.
 * `stroke-width` → `strokeWidth`), verified byte-for-byte in plugins.test.ts.
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
  attributes: {
    ...defaultSchema.attributes,
    span: [
      ...(defaultSchema.attributes?.span ?? []),
      ["className", "math", "math-inline", "math-display"],
    ],
    // Presentation attributes are inert, so admitting them globally (rather
    // than per-SVG-tag) keeps the list readable without opening any HTML
    // vector — a stray `fill` on a `<div>` does nothing.
    "*": [...(defaultSchema.attributes?.["*"] ?? []), ...SVG_ATTRS],
  },
};

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
 */
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
    buf.push(line);
    depth += (line.match(/<svg\b/g) ?? []).length;
    depth -= (line.match(/<\/svg\s*>/g) ?? []).length;
    if (depth <= 0) flush(true); // balanced close → safe to drop inner blanks
  }
  if (buf.length) flush(false); // reached EOF mid-span → unbalanced, keep blanks
  return out.join("\n");
}

/**
 * `remark-cjk-friendly` is load-bearing, not a nicety: CommonMark refuses to
 * open emphasis when the `**` sits between a CJK character and punctuation, so
 * `一个是**“引号开头”的加粗**` renders the asterisks literally (verified against
 * this exact chain). The fix has to live in the tokenizer — by the time a mdast
 * plugin could see the tree, the `**` is already plain text.
 */
export const safeRemarkPlugins: PluggableList = [
  // A bare home path starts with `~`. With remark-gfm's permissive default,
  // two paths such as `~/.claude/skills` and `~/.codex/skills` can swallow
  // everything between them into a <del>. GFM's standard `~~text~~` form
  // remains enabled when the single-tilde extension is disabled.
  [remarkGfm, { singleTilde: false }],
  // A single `\n` (soft break) renders as a real line break, not a space —
  // kept in step with the mobile-web chain (mobile-web/src/markdown/plugins.ts).
  remarkBreaks,
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
  // Runs last so the `cjk-indent` class it adds to CJK-leading <p> survives the
  // sanitize pass above (className is globally allow-listed by `schema`).
  rehypeCjkIndent,
];
