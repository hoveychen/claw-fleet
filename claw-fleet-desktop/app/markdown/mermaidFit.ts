/**
 * 宽 mermaid 图不该被缩到读不了。
 *
 * mermaid 吐出的 svg 是 `width="100%"` + `style="max-width:<自然宽>px"` + viewBox。
 * 于是它总是撑满容器，viewBox 再把内容整体缩到 `容器宽 / 自然宽`。实测知识库面板
 * 宽 524px、一张架构图自然宽 1279.5px —— 缩到 41%，14px 的标签渲染成 5.7px。
 * `.diagram` 上那句 `overflow-x: auto` 从来没生效过，因为图永远不溢出。
 *
 * 这里给缩放定一个下限：能装下就照常缩（窄图仍然完整显示），装不下就按下限画，
 * 让它溢出容器、交给 `overflow-x: auto` 横向滚动。
 *
 * 桌面端和移动端各有一份（和 mermaidContrast.ts / mermaidTheme.ts 同样的约定）。
 */

/** 允许缩到的最小比例。低于这个字就开始糊了。 */
export const MIN_DIAGRAM_SCALE = 0.7;

/** 从 `viewBox="minX minY w h"` 里取自然宽；取不到返回 null。 */
export function naturalWidthFromViewBox(viewBox: string | null): number | null {
  if (!viewBox) return null;
  const parts = viewBox.trim().split(/[\s,]+/);
  if (parts.length < 4) return null;
  const w = Number.parseFloat(parts[2]);
  return Number.isFinite(w) && w > 0 ? w : null;
}

/**
 * 该给这张图钉多宽（px），`null` 表示别管它、维持 mermaid 自己的 100% 行为。
 *
 * - 容器装得下自然宽 → null（mermaid 的 max-width 会把它停在自然宽，不会放大）
 * - 装不下但缩放还在下限以上 → null（照常缩，窄图完整显示）
 * - 缩放会掉到下限以下 → 返回 `自然宽 × 下限`，溢出容器交给横向滚动
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
 * viewBox 会把整张图连字一起放大到容器宽（实测：阅读模式里自然宽 135px 的窄流程图
 * 被画成 778px，5.78 倍，节点里的字大到溢出方框）。
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
