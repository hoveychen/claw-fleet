// Mirrors the desktop chain (claw-fleet-desktop/app/markdown/plugins.ts) — the
// two apps are separate vite packages, so the list is duplicated rather than
// shared. Keep them in step: a message that bolds on the desktop must bold on
// the phone.
import type { PluggableList } from "unified";
import remarkGfm from "remark-gfm";
import remarkBreaks from "remark-breaks";
import remarkCjkFriendly from "remark-cjk-friendly";
import remarkMath from "remark-math";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeKatex from "rehype-katex";
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
  attributes: {
    ...defaultSchema.attributes,
    span: [
      ...(defaultSchema.attributes?.span ?? []),
      ["className", "math", "math-inline", "math-display"],
    ],
    "*": [...(defaultSchema.attributes?.["*"] ?? []), ...SVG_ATTRS],
  },
};

/**
 * `remark-cjk-friendly` is what makes `一个是**“引号开头”的加粗**` bold at all:
 * CommonMark won't open emphasis when `**` sits between a CJK character and
 * punctuation, and the fix has to happen in the tokenizer.
 */
export const mdRemarkPlugins: PluggableList = [
  remarkGfm,
  // A single `\n` (soft break) renders as a real line break, not a space — handoff
  // notes and chat messages often lean on bare newlines instead of blank lines.
  remarkBreaks,
  remarkCjkFriendly,
  remarkMath,
];

/** raw → sanitize → katex: scrub the model's HTML, then emit KaTeX's trusted DOM. */
export const mdRehypePlugins: PluggableList = [
  rehypeRaw,
  [rehypeSanitize, schema],
  rehypeKatex,
];
