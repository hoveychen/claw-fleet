// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  MIN_DIAGRAM_SCALE,
  applyDiagramWidth,
  fitDiagramWidth,
  naturalWidthFromViewBox,
} from "./mermaidFit";

describe("naturalWidthFromViewBox", () => {
  it("read the viewBox format that mermaid actually outputs", () => {
    // Measured value: the arch/overview diagram in the wiki.
    expect(naturalWidthFromViewBox("0 0 1279.546875 224.28900146484375")).toBeCloseTo(
      1279.546875,
    );
  });

  it("comma-separated values also work", () => {
    expect(naturalWidthFromViewBox("0,0,800,200")).toBe(800);
  });

  it("missing, incomplete, or invalid width returns null to skip diagram processing", () => {
    expect(naturalWidthFromViewBox(null)).toBeNull();
    expect(naturalWidthFromViewBox("0 0 800")).toBeNull();
    expect(naturalWidthFromViewBox("0 0 abc 200")).toBeNull();
    expect(naturalWidthFromViewBox("0 0 0 200")).toBeNull();
  });
});

describe("fitDiagramWidth", () => {
  it("container fits natural width — no adjustment needed", () => {
    expect(fitDiagramWidth(600, 900)).toBeNull();
    expect(fitDiagramWidth(600, 600)).toBeNull();
  });

  it("too large but scaled value stays above minimum — no adjustment", () => {
    // 800 × 0.7 = 560, container 700 is wider.
    expect(fitDiagramWidth(800, 700)).toBeNull();
  });

  it("lands exactly on minimum — no adjustment to avoid boundary oscillation", () => {
    expect(fitDiagramWidth(800, 560)).toBeNull();
  });

  it("scaled value falls below minimum — pin to minimum, overflow container", () => {
    // Real case: natural width 1279.5, container 524 → originally 41%.
    const w = fitDiagramWidth(1279.546875, 524);
    expect(w).toBeCloseTo(1279.546875 * MIN_DIAGRAM_SCALE);
    expect(w! / 1279.546875).toBeCloseTo(MIN_DIAGRAM_SCALE);
    expect(w!).toBeGreaterThan(524); // overflow enables horizontal scroll
  });

  it("minimum scale is configurable", () => {
    expect(fitDiagramWidth(1000, 300, 0.5)).toBe(500);
    expect(fitDiagramWidth(1000, 300, 1)).toBe(1000);
  });

  it("cannot measure natural width or container not laid out — no adjustment", () => {
    expect(fitDiagramWidth(null, 524)).toBeNull();
    expect(fitDiagramWidth(1000, 0)).toBeNull();
  });
});

/** Create an SVG in the shape that mermaid just output: width="100%" + inline max-width=natural width.
 *  Both are written by setupViewPortForSVG → configureSvgSize; the viewBox width and max-width
 *  values share the same source, so they are always equal. */
function mermaidSvg(naturalWidth: number): SVGElement {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", `0 0 ${naturalWidth} 200`);
  el.setAttribute("width", "100%");
  el.setAttribute("style", `max-width: ${naturalWidth}px;`);
  return el;
}

describe("applyDiagramWidth", () => {
  it("when container fits, preserve the natural width max-width set by mermaid", () => {
    // Removing it leaves svg with only width="100%", stretched to fill container:
    // measured in reader: a 135px-wide flowchart becomes 778px (5.78×), text scaled too.
    const el = mermaidSvg(135);
    applyDiagramWidth(el, 778);
    expect(el.style.maxWidth).toBe("135px");
    expect(el.style.width).toBe("");
  });

  it("too large and over-scaled — pin to minimum width, clear inline max-width", () => {
    const el = mermaidSvg(1000);
    applyDiagramWidth(el, 300);
    expect(el.style.width).toBe(`${1000 * MIN_DIAGRAM_SCALE}px`);
    expect(el.style.maxWidth).toBe("none");
  });

  it("transition from pinned to fitting (split wider / sidebar expanded) — restore natural width", () => {
    const el = mermaidSvg(1000);
    applyDiagramWidth(el, 300); // pin first
    applyDiagramWidth(el, 1200); // then release
    expect(el.style.width).toBe("");
    expect(el.style.maxWidth).toBe("1000px");
  });

  it("no viewBox — do nothing at all", () => {
    const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    el.setAttribute("style", "max-width: 400px;");
    applyDiagramWidth(el, 778);
    expect(el.style.maxWidth).toBe("400px");
  });
});
