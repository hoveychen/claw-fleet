// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  MIN_DIAGRAM_SCALE,
  applyDiagramWidth,
  fitDiagramWidth,
  naturalWidthFromViewBox,
} from "./mermaidFit";

describe("naturalWidthFromViewBox", () => {
  it("读 mermaid 实际吐出的那种 viewBox", () => {
    // 实测值：知识库 arch/overview 那张架构图。
    expect(naturalWidthFromViewBox("0 0 1279.546875 224.28900146484375")).toBeCloseTo(
      1279.546875,
    );
  });

  it("逗号分隔也认", () => {
    expect(naturalWidthFromViewBox("0,0,800,200")).toBe(800);
  });

  it("缺席 / 残缺 / 非法宽度都返回 null，让调用方别管这张图", () => {
    expect(naturalWidthFromViewBox(null)).toBeNull();
    expect(naturalWidthFromViewBox("0 0 800")).toBeNull();
    expect(naturalWidthFromViewBox("0 0 abc 200")).toBeNull();
    expect(naturalWidthFromViewBox("0 0 0 200")).toBeNull();
  });
});

describe("fitDiagramWidth", () => {
  it("容器装得下自然宽 → 不插手", () => {
    expect(fitDiagramWidth(600, 900)).toBeNull();
    expect(fitDiagramWidth(600, 600)).toBeNull();
  });

  it("装不下但缩放还在下限以上 → 不插手，照常缩", () => {
    // 800 × 0.7 = 560，容器 700 还宽于它。
    expect(fitDiagramWidth(800, 700)).toBeNull();
  });

  it("刚好落在下限上 → 不插手（边界不来回抖）", () => {
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

/** 造一个 mermaid 刚吐出来那个形状的 svg：width="100%" + 内联 max-width=自然宽。
 *  这两样是 setupViewPortForSVG → configureSvgSize 写的，viewBox 的宽和 max-width
 *  的数值同源，所以永远相等。 */
function mermaidSvg(naturalWidth: number): SVGElement {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", `0 0 ${naturalWidth} 200`);
  el.setAttribute("width", "100%");
  el.setAttribute("style", `max-width: ${naturalWidth}px;`);
  return el;
}

describe("applyDiagramWidth", () => {
  it("容器装得下时，必须留着 mermaid 写在 svg 上的自然宽 max-width", () => {
    // 抹掉它，svg 就只剩 width="100%"，会被拉满整个容器：实测阅读页里一张自然宽
    // 135px 的窄流程图被画成 778px（5.78 倍），字跟着一起放大。
    const el = mermaidSvg(135);
    applyDiagramWidth(el, 778);
    expect(el.style.maxWidth).toBe("135px");
    expect(el.style.width).toBe("");
  });

  it("装不下且缩过头 → 钉到下限宽并让内联 max-width 让路", () => {
    const el = mermaidSvg(1000);
    applyDiagramWidth(el, 300);
    expect(el.style.width).toBe(`${1000 * MIN_DIAGRAM_SCALE}px`);
    expect(el.style.maxWidth).toBe("none");
  });

  it("从钉宽回到装得下（分屏拉宽 / 侧栏展开）→ 自然宽要还回去", () => {
    const el = mermaidSvg(1000);
    applyDiagramWidth(el, 300); // 先钉
    applyDiagramWidth(el, 1200); // 再放开
    expect(el.style.width).toBe("");
    expect(el.style.maxWidth).toBe("1000px");
  });

  it("没有 viewBox 就完全不插手", () => {
    const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    el.setAttribute("style", "max-width: 400px;");
    applyDiagramWidth(el, 778);
    expect(el.style.maxWidth).toBe("400px");
  });
});
