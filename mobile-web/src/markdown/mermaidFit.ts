/**
 * Wide mermaid diagrams shouldn't scale to unreadability.
 *
 * Mermaid's SVG is `width="100%"` + `style="max-width:<natural-width>px"` + viewBox.
 * It always stretches to fill the container; viewBox then scales content to
 * `container-width / natural-width`. Test case: wiki panel is 524px wide, an architecture
 * diagram naturally 1279.5px wide — scales to 41%, rendering 14px labels as 5.7px.
 * The `.diagram` `overflow-x: auto` never works because the diagram never overflows.
 *
 * Set a floor on scaling: if it fits, scale normally (narrow diagrams stay complete);
 * if not, pin at the floor, let it overflow, and let `overflow-x: auto` scroll it.
 *
 * Desktop and mobile each have one (same convention as mermaidContrast.ts / mermaidTheme.ts).
 */

/** Minimum allowed scale. Below this, text becomes blurry. */
export const MIN_DIAGRAM_SCALE = 0.7;

/** Extract natural width from `viewBox="minX minY w h"`; returns null if not found. */
export function naturalWidthFromViewBox(viewBox: string | null): number | null {
  if (!viewBox) return null;
  const parts = viewBox.trim().split(/[\s,]+/);
  if (parts.length < 4) return null;
  const w = Number.parseFloat(parts[2]);
  return Number.isFinite(w) && w > 0 ? w : null;
}

/**
 * How wide (px) to pin this diagram; `null` means leave it alone, keep mermaid's 100% behavior.
 *
 * - Container fits natural width → `null` (mermaid's max-width stops it there, won't enlarge)
 * - Doesn't fit but scale stays above floor → `null` (scale normally, narrow diagrams stay complete)
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
 * Apply the above calculation to the SVG. Placed here instead of in a component to allow direct testing
 * in jsdom.
 *
 * Mermaid's SVG is `width="100%"` + **inline** `style="max-width:<natural-width>px"` (from
 * setupViewPortForSVG → configureSvgSize, this value sources from viewBox width). That inline
 * max-width is the only thing stopping the diagram from stretching to container width — stylesheet
 * `.diagram svg{max-width:100%}` can't override it. Once `removeProperty("max-width")` removes it,
 * the SVG has only `width="100%"`, and viewBox scales everything to container width (desktop testing:
 * a 135px narrow flowchart became 778px, 5.78× scale, text overflowing nodes).
 *
 * So the "do nothing" path must write the natural width **back**, not delete it.
 */
export function applyDiagramWidth(el: SVGElement, containerWidth: number): void {
  const natural = naturalWidthFromViewBox(el.getAttribute("viewBox"));
  // If we can't measure natural width, don't touch anything: since we didn't measure it, the
  // pinned-width path below never runs, so there's nothing to undo.
  if (natural === null) return;
  const pinned = fitDiagramWidth(natural, containerWidth);
  if (pinned === null) {
    el.style.removeProperty("width");
    el.style.maxWidth = `${natural}px`;
    return;
  }
  // When pinning width, max-width must also be cleared, or mermaid's natural width will pull it back.
  el.style.width = `${pinned}px`;
  el.style.maxWidth = "none";
}
