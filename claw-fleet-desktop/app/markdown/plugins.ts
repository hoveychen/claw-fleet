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
