import { describe, expect, it } from "vitest";
import {
  MIN_DIAGRAM_SCALE,
  fitDiagramWidth,
  naturalWidthFromViewBox,
} from "./mermaidFit";

describe("naturalWidthFromViewBox", () => {
  it("reads the viewBox that mermaid actually emits", () => {
    // Measured from wiki arch/overview diagram.
    expect(naturalWidthFromViewBox("0 0 1279.546875 224.28900146484375")).toBeCloseTo(
      1279.546875,
    );
  });

  it("also accepts comma-separated values", () => {
    expect(naturalWidthFromViewBox("0,0,800,200")).toBe(800);
  });

  it("returns null for missing / truncated / invalid widths, so caller ignores diagram", () => {
    expect(naturalWidthFromViewBox(null)).toBeNull();
    expect(naturalWidthFromViewBox("0 0 800")).toBeNull();
    expect(naturalWidthFromViewBox("0 0 abc 200")).toBeNull();
    expect(naturalWidthFromViewBox("0 0 0 200")).toBeNull();
  });
});

describe("fitDiagramWidth", () => {
  it("container fits natural width → no scaling needed", () => {
    expect(fitDiagramWidth(600, 900)).toBeNull();
    expect(fitDiagramWidth(600, 600)).toBeNull();
  });

  it("doesn't fit but scaling stays above minimum → don't intervene", () => {
    // 800 × 0.7 = 560, container 700 is still wider.
    expect(fitDiagramWidth(800, 700)).toBeNull();
  });

  it("exactly at minimum → don't intervene (no boundary jitter)", () => {
    expect(fitDiagramWidth(800, 560)).toBeNull();
  });

  it("缩放会掉到下限以下 → 钉到下限宽度，溢出容器", () => {
    // 老板遇到的那张：自然宽 1279.5，容器 524 → 原本 41%。
    const w = fitDiagramWidth(1279.546875, 524);
    expect(w).toBeCloseTo(1279.546875 * MIN_DIAGRAM_SCALE);
    expect(w! / 1279.546875).toBeCloseTo(MIN_DIAGRAM_SCALE);
    expect(w!).toBeGreaterThan(524); // 溢出才有横向滚动
  });

  it("下限可调", () => {
    expect(fitDiagramWidth(1000, 300, 0.5)).toBe(500);
    expect(fitDiagramWidth(1000, 300, 1)).toBe(1000);
  });

  it("量不到自然宽或容器还没布局 → 不插手", () => {
    expect(fitDiagramWidth(null, 524)).toBeNull();
    expect(fitDiagramWidth(1000, 0)).toBeNull();
  });
});
