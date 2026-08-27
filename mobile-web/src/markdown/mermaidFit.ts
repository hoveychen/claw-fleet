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
