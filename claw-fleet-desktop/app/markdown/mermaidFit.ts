/**
 * Wide mermaid diagrams should not be shrunk to illegibility.
 *
 * Mermaid emits SVG as `width="100%"` + `style="max-width:<natural-width>px"` + viewBox.
 * This always fills the container, and viewBox scales the content down by `container-width / natural-width`.
 * In practice: wiki panel width 524px, diagram natural width 1279.5px → scales to 41%, turning 14px labels into 5.7px.
 * The `overflow-x: auto` on `.diagram` never worked because the diagram never overflows.
 *
 * Set a floor on the scaling: if it fits, scale normally (narrow diagrams stay whole); if not, pin the width
 * to the floor, let it overflow the container, and let `overflow-x: auto` scroll it.
 *
 * Both desktop and mobile have copies (same convention as mermaidContrast.ts / mermaidTheme.ts).
 */

/** Minimum scale ratio allowed. Text becomes unreadable below this. */
export const MIN_DIAGRAM_SCALE = 0.7;

/** Extract natural width from `viewBox="minX minY w h"`; null if not found. */
export function naturalWidthFromViewBox(viewBox: string | null): number | null {
  if (!viewBox) return null;
  const parts = viewBox.trim().split(/[\s,]+/);
  if (parts.length < 4) return null;
  const w = Number.parseFloat(parts[2]);
  return Number.isFinite(w) && w > 0 ? w : null;
}

/**
 * How wide (px) should this diagram be? null = let it be, maintain mermaid's own 100% behavior.
 *
 * - Container fits the natural width → null (mermaid's max-width pins it to natural width, no enlargement)
 * - Does not fit but scale stays above floor → null (scale normally, narrow diagram stays whole)
 * - Scale would drop below floor → return `natural-width × floor`, overflow to horizontal scroll
 */
export function fitDiagramWidth(
  naturalWidth: number | null,
  containerWidth: number,
  minScale: number = MIN_DIAGRAM_SCALE,
): number | null {
  if (naturalWidth === null || naturalWidth <= 0) return null;
  if (containerWidth <= 0) return null;
  const floor = naturalWidth * minScale;
  return containerWidth < floor ? floor : null;
}

/**
 * Apply the calculated width to the SVG. Kept here rather than in a component so it can be tested directly in jsdom.
 *
 * Mermaid emits SVG as `width="100%"` + **inline** `style="max-width:<natural-width>px"`
 * (setupViewPortForSVG → configureSvgSize; this value comes from viewBox width).
 * That inline max-width is the only thing stopping the diagram from being pulled to fill the container —
 * the stylesheet `.diagram svg{max-width:100%}` can't override it. Once it's removed via `removeProperty("max-width")`,
 * the SVG is left with only `width="100%"`, and viewBox scales the entire diagram up to container width
 * (real case: reading mode, narrow flowchart natural width 135px drawn as 778px, 5.78×, text overflowed boxes).
 *
 * So the "don't touch" code path must write the natural width back, not delete it.
 */
export function applyDiagramWidth(el: SVGElement, containerWidth: number): void {
  const natural = naturalWidthFromViewBox(el.getAttribute("viewBox"));
  // Can't measure natural width → don't touch at all: the pinning path never ran either, so nothing to undo.
  if (natural === null) return;
  const pinned = fitDiagramWidth(natural, containerWidth);
  if (pinned === null) {
    el.style.removeProperty("width");
    el.style.maxWidth = `${natural}px`;
    return;
  }
  // When pinning width, max-width must get out of the way too, else mermaid's natural-width wins.
  el.style.width = `${pinned}px`;
  el.style.maxWidth = "none";
}
