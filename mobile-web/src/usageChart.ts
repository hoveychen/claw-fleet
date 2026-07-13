// 「占用率变化」曲线的纯几何：把 0–1 的采样点映射成一条 SVG path。
// 单独拆出来是为了能直接单测——SVG 元素本身没什么好测的，容易出错的是坐标映射。
// 不引图表库：桌面端那张图用 recharts，手机上只要两条折线，手写更省一个依赖。

import type { UsageHistoryPoint } from "./types";

/** 画布坐标系（SVG viewBox 的用户单位）与时间窗。 */
export interface ChartBox {
  width: number;
  height: number;
  fromMs: number;
  toMs: number;
}

/** 从一个采样点里取某条曲线的值；该窗口当次没数据时返回 null。 */
export type PickMetric = (p: UsageHistoryPoint) => number | null;

/** 折线 path。采样点不足两个（连不成线）返回空串。
 *  null 采样直接跳过、两侧点直连——与桌面端 recharts 的 connectNulls 同行为。 */
export function linePath(
  points: UsageHistoryPoint[],
  pick: PickMetric,
  box: ChartBox,
): string {
  const span = Math.max(1, box.toMs - box.fromMs);
  const coords = points
    .map((p) => ({ ts: p.ts, v: pick(p) }))
    .filter((p): p is { ts: number; v: number } => p.v !== null && p.v !== undefined)
    .sort((a, b) => a.ts - b.ts)
    .map(({ ts, v }) => {
      const x = ((ts - box.fromMs) / span) * box.width;
      // SVG 的 y 轴朝下：占用率 100% 落在顶边。
      const y = box.height - clamp01(v) * box.height;
      return `${round1(x)},${round1(y)}`;
    });
  if (coords.length < 2) return "";
  return `M${coords.join("L")}`;
}

/** 时间轴刻度：窗口内每隔 `stepMs` 一个，返回 [x 坐标, 时间戳]。 */
export function timeTicks(box: ChartBox, stepMs: number): Array<[number, number]> {
  const span = Math.max(1, box.toMs - box.fromMs);
  const ticks: Array<[number, number]> = [];
  // 从窗口右端(现在)往回取整步长，最后一个刻度就总是落在“现在”附近。
  for (let ts = box.toMs; ts >= box.fromMs; ts -= stepMs) {
    ticks.push([round1n(((ts - box.fromMs) / span) * box.width), ts]);
  }
  return ticks.reverse();
}

function clamp01(v: number): number {
  return Math.min(Math.max(v, 0), 1);
}

function round1(n: number): string {
  return String(round1n(n));
}

function round1n(n: number): number {
  return Math.round(n * 10) / 10;
}
