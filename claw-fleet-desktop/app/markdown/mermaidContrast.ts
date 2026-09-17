/**
 * Fix the black blotches in mermaid diagrams caused by "author hard-coded fill
 * but label color still defaults to theme".
 *
 * Mermaid writes `style X fill:#4a3728` as inline `fill` on the node shape, but
 * label color comes from the theme's inline stylesheet (default theme is `#333`,
 * dark theme is light). A diagram styled for dark theme becomes dark text on dark
 * background in light theme — the whole node becomes a black blob.
 *
 * After rendering, we apply a fix: for any node/subgraph with author-specified fill
 * but no explicit label color, flip the label to black or white based on fill
 * brightness. Nodes without specified fill are left alone (theme defaults are
 * correct anyway), and nodes with explicit `color:` are left alone (that's the
 * author's choice).
 */

/** Complement candidates: dark enough but not pure black, light enough but not garish. */
const LIGHT_INK = "#f5f5f5";
const DARK_INK = "#1a1a1a";

type Rgb = [number, number, number];

/** Only parse the two formats mermaid actually emits: `#rgb`/`#rrggbb` and `rgb()/rgba()`. */
export function parseColor(css: string): Rgb | null {
  const s = css.trim().toLowerCase();
  if (s === "" || s === "none" || s === "transparent") return null;
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/.exec(s);
  if (hex) {
    const h = hex[1];
    const wide =
      h.length === 3
        ? h
            .split("")
            .map((c) => c + c)
            .join("")
        : h;
    return [
      parseInt(wide.slice(0, 2), 16),
      parseInt(wide.slice(2, 4), 16),
      parseInt(wide.slice(4, 6), 16),
    ];
  }
  const fn = /^rgba?\(([^)]+)\)$/.exec(s);
  if (fn) {
    const parts = fn[1].split(/[\s,/]+/).filter((p) => p !== "");
    if (parts.length < 3) return null;
    const nums = parts.slice(0, 3).map((p) => Number.parseFloat(p));
    if (nums.some((n) => Number.isNaN(n))) return null;
    return [nums[0], nums[1], nums[2]] as Rgb;
  }
  return null;
}

/** WCAG relative luminance. */
function luminance([r, g, b]: Rgb): number {
  const lin = [r, g, b].map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
}

/** WCAG contrast ratio, from 1 (identical color) to 21 (black and white). */
export function contrastRatio(a: string, b: string): number {
  const ca = parseColor(a);
  const cb = parseColor(b);
  if (!ca || !cb) return 1;
  const la = luminance(ca);
  const lb = luminance(cb);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** Given a background color, pick ink color with higher contrast; return null if background cannot be parsed (leave it alone). */
export function legibleInkFor(fill: string): string | null {
  if (!parseColor(fill)) return null;
  return contrastRatio(fill, LIGHT_INK) >= contrastRatio(fill, DARK_INK)
    ? LIGHT_INK
    : DARK_INK;
}

/** Extract a declaration value from the `style` attribute, stripping `!important` along the way. */
function declaration(el: Element, prop: string): string | null {
  const raw = el.getAttribute("style");
  if (!raw) return null;
  for (const decl of raw.split(";")) {
    const at = decl.indexOf(":");
    if (at < 0) continue;
    if (decl.slice(0, at).trim().toLowerCase() !== prop) continue;
    return decl
      .slice(at + 1)
      .replace(/!important/i, "")
      .trim();
  }
  return null;
}

const SHAPE = [
  ":scope > rect",
  ":scope > polygon",
  ":scope > circle",
  ":scope > ellipse",
  ":scope > path",
].join(",");

const LABEL = [
  ":scope > g.label span",
  ":scope > g.label p",
  ":scope > g.label text",
  ":scope > g.label tspan",
  ":scope > g.cluster-label span",
  ":scope > g.cluster-label p",
  ":scope > g.cluster-label text",
  ":scope > g.cluster-label tspan",
].join(",");

/**
 * Fix a rendered mermaid SVG in-place (root is the container wrapping `<svg>`
 * or the svg itself). Idempotent: running again just writes the same colors again.
 */
export function repairMermaidLabelContrast(root: ParentNode): void {
  for (const group of Array.from(root.querySelectorAll("g.node, g.cluster"))) {
    const shape = group.querySelector(SHAPE);
    if (!shape) continue;
    const fill = declaration(shape, "fill");
    // No author-specified fill — theme-bundled colors and label colors are already paired, don't interfere.
    if (!fill) continue;
    const ink = legibleInkFor(fill);
    if (!ink) continue;
    for (const label of Array.from(group.querySelectorAll(LABEL))) {
      // Author wrote `color:` themselves; mermaid inlines it onto the label — that's their choice.
      // Only the two ink colors we added in the previous pass are allowed to be rewritten (for idempotence).
      const own = declaration(label, "color") ?? declaration(label, "fill");
      if (own !== null && own !== LIGHT_INK && own !== DARK_INK) continue;
      const prior = label.getAttribute("style") ?? "";
      const kept = prior
        .split(";")
        .filter((d) => {
          const name = d.slice(0, d.indexOf(":")).trim().toLowerCase();
          return d.trim() !== "" && name !== "color" && name !== "fill";
        })
        .join(";");
      const patch = `color:${ink} !important;fill:${ink} !important`;
      label.setAttribute("style", kept === "" ? patch : `${kept};${patch}`);
    }
  }
}

/**
 * Same fix, but applied to the mermaid-emitted SVG as a **string**.
 *
 * Modifying the DOM after mounting is fragile: the decision-card path actually
 * re-injects the same SVG once after the effect runs, wiping out the label
 * colors we fixed (the `data-repair` attribute remains, but the spans revert
 * to unstyled). By baking the colors into the string, any re-injection still
 * has the fixed version.
 */
export function repairMermaidContrastInSvg(svgText: string): string {
  if (typeof document === "undefined") return svgText;
  // Use HTML parser, not XML parser: mermaid's multi-line labels contain bare `<br>`,
  // and `DOMParser(..., "image/svg+xml")` treats them as a parse error and skips the
  // whole fix (verified: the desktop wiki diagram was affected this way). innerHTML
  // uses the lenient HTML parsing path, same as React's when it injects this SVG.
  const host = document.createElement("div");
  host.innerHTML = svgText;
  const svg = host.querySelector("svg");
  if (!svg) return svgText;
  repairMermaidLabelContrast(svg);
  return host.innerHTML;
}
