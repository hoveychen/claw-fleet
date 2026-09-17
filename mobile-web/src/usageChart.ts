// Pure geometry of the "usage over time" curve: map normalized (0–1) sample
// points to an SVG path. Extracted separately for direct testing — SVG elements
// themselves aren't interesting to test, the coordinate mapping is error-prone.
// No chart library: desktop uses recharts, mobile only needs two polylines, so
// hand-coding saves a dependency.

/** Canvas coordinate system (SVG viewBox user units) and time window. */
export interface ChartBox {
  width: number;
  height: number;
  fromMs: number;
  toMs: number;
}

/** Extract a metric value from a sample point (**0–1** normalized); returns
 *  null if no data for this window. Once generic, both Claude
 *  (`UsageHistoryPoint`) and codex (`CodexUsageHistoryPoint`, /100 in pick)
 *  sample types can share the same geometry. */
export type PickMetric<T extends { ts: number }> = (p: T) => number | null;

/** Polyline path. Returns empty string if fewer than two sample points (can't
 *  draw a line). Null samples are skipped and adjacent points are connected
 *  directly — same behavior as recharts' connectNulls on desktop. */
export function linePath<T extends { ts: number }>(
  points: T[],
  pick: PickMetric<T>,
  box: ChartBox,
): string {
  const span = Math.max(1, box.toMs - box.fromMs);
  const coords = points
    .map((p) => ({ ts: p.ts, v: pick(p) }))
    .filter((p): p is { ts: number; v: number } => p.v !== null && p.v !== undefined)
    .sort((a, b) => a.ts - b.ts)
    .map(({ ts, v }) => {
      const x = ((ts - box.fromMs) / span) * box.width;
      // SVG y-axis points down: 100% usage lands at the top edge.
      const y = box.height - clamp01(v) * box.height;
      return `${round1(x)},${round1(y)}`;
    });
  if (coords.length < 2) return "";
  return `M${coords.join("L")}`;
}

/** Time axis ticks: one every `stepMs` within the window. Returns [x coordinate,
 *  timestamp]. */
export function timeTicks(box: ChartBox, stepMs: number): Array<[number, number]> {
  const span = Math.max(1, box.toMs - box.fromMs);
  const ticks: Array<[number, number]> = [];
  // Walk back from the window's right edge (now) in full steps; the last tick
  // always lands near “now”.
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
