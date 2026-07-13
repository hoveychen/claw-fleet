import { describe, expect, it } from "vitest";
import { linePath, timeTicks, type ChartBox } from "./usageChart";
import type { UsageHistoryPoint } from "./types";

const BOX: ChartBox = { width: 100, height: 50, fromMs: 0, toMs: 1000 };

function pt(ts: number, fiveHour: number | null): UsageHistoryPoint {
  return { ts, fiveHour, sevenDay: null, sevenDaySonnet: null };
}

describe("linePath", () => {
  it("maps time to x and 0–1 occupancy to an inverted y", () => {
    // 窗口起点 0% → 左下角；窗口终点 100% → 右上角。
    const path = linePath([pt(0, 0), pt(1000, 1)], (p) => p.fiveHour, BOX);
    expect(path).toBe("M0,50L100,0");
  });

  it("skips null samples and connects across the hole", () => {
    const path = linePath(
      [pt(0, 0.5), pt(500, null), pt(1000, 0.5)],
      (p) => p.fiveHour,
      BOX,
    );
    expect(path).toBe("M0,25L100,25");
  });

  it("returns an empty path when fewer than two samples have data", () => {
    expect(linePath([], (p) => p.fiveHour, BOX)).toBe("");
    expect(linePath([pt(0, 0.4)], (p) => p.fiveHour, BOX)).toBe("");
    expect(linePath([pt(0, 0.4), pt(500, null)], (p) => p.fiveHour, BOX)).toBe("");
  });

  it("clamps out-of-range values into the plot area", () => {
    const path = linePath([pt(0, -0.2), pt(1000, 1.4)], (p) => p.fiveHour, BOX);
    expect(path).toBe("M0,50L100,0");
  });

  it("sorts samples that arrive out of order", () => {
    const path = linePath([pt(1000, 1), pt(0, 0)], (p) => p.fiveHour, BOX);
    expect(path).toBe("M0,50L100,0");
  });
});

describe("timeTicks", () => {
  it("anchors the last tick on the window's end and steps backwards", () => {
    expect(timeTicks(BOX, 500)).toEqual([
      [0, 0],
      [50, 500],
      [100, 1000],
    ]);
  });
});
