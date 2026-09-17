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

/** Extract natural width from `viewBox="minX minY w h"`; return null if not found. */
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
 * 把上面算出来的结论落到 svg 上。挂在这里而不是组件里，是为了能在 jsdom 里直接测。
 *
 * mermaid 吐出来的 svg 是 `width="100%"` + **内联** `style="max-width:<自然宽>px"`
 * （setupViewPortForSVG → configureSvgSize，那个数和 viewBox 的宽同源）。那句内联
 * max-width 是唯一拦着图别被拉满容器的东西 —— 样式表里的 `.diagram svg{max-width:100%}`
 * 压不过它，一旦被 `removeProperty("max-width")` 抹掉，svg 就只剩 `width="100%"`，
 * viewBox 会把整张图连字一起放大到容器宽（桌面端实测：阅读模式里自然宽 135px 的窄
 * 流程图被画成 778px，5.78 倍，节点里的字大到溢出方框）。
 *
 * 所以"不插手"这条路径必须把自然宽**写回去**，而不是删掉。
 */
export function applyDiagramWidth(el: SVGElement, containerWidth: number): void {
  const natural = naturalWidthFromViewBox(el.getAttribute("viewBox"));
  // 量不到自然宽就彻底不碰：既然没量到，下面那条钉宽路径也从没走过，无需撤销。
  if (natural === null) return;
  const pinned = fitDiagramWidth(natural, containerWidth);
  if (pinned === null) {
    el.style.removeProperty("width");
    el.style.maxWidth = `${natural}px`;
    return;
  }
  // 钉宽时 max-width 必须一起让路，否则 mermaid 那句自然宽会把它拽回去。
  el.style.width = `${pinned}px`;
  el.style.maxWidth = "none";
}
