// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  contrastRatio,
  legibleInkFor,
  parseColor,
  repairMermaidContrastInSvg,
  repairMermaidLabelContrast,
} from "./mermaidContrast";

/**
 * Replicate the real node structure that mermaid 11 produces (dumped from the probe page in browser):
 * shapes are direct children of `g.node` with inline fill, labels are spans inside foreignObject,
 * colors come from svg inline stylesheets (default theme `#333`), span itself has no inline color.
 */
function node(opts: {
  tag?: "rect" | "polygon";
  shapeStyle?: string;
  labelStyle?: string;
  cluster?: boolean;
}): SVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const g = document.createElementNS(ns, "g");
  g.setAttribute("class", opts.cluster ? "cluster" : "node default");
  const shape = document.createElementNS(ns, opts.tag ?? "rect");
  if (opts.shapeStyle) shape.setAttribute("style", opts.shapeStyle);
  g.appendChild(shape);
  const label = document.createElementNS(ns, "g");
  label.setAttribute("class", opts.cluster ? "cluster-label" : "label");
  const fo = document.createElementNS(ns, "foreignObject");
  const span = document.createElement("span");
  span.setAttribute("class", "nodeLabel");
  if (opts.labelStyle) span.setAttribute("style", opts.labelStyle);
  span.textContent = "④ Canonicalize";
  fo.appendChild(span);
  label.appendChild(fo);
  g.appendChild(label);
  return g;
}

function mount(...groups: SVGElement[]): SVGSVGElement {
  const svg = document.createElementNS(
    "http://www.w3.org/2000/svg",
    "svg",
  ) as SVGSVGElement;
  for (const g of groups) svg.appendChild(g);
  document.body.replaceChildren(svg);
  return svg;
}

function ink(g: Element): string | null {
  const style = g.querySelector("span")?.getAttribute("style") ?? "";
  const m = /(?:^|;)\s*color\s*:\s*([^;!]+)/.exec(style);
  return m ? m[1].trim() : null;
}

/** Assert that a node's label color has sufficient contrast with its background color. */
function expectLegible(g: Element, fill: string): void {
  const got = ink(g);
  expect(got, "标签没有被补上可读的颜色").not.toBeNull();
  expect(contrastRatio(fill, got!)).toBeGreaterThan(4.5);
}

describe("parseColor", () => {
  it("recognizes #rgb / #rrggbb / rgb() three formats", () => {
    expect(parseColor("#4a3728")).toEqual([74, 55, 40]);
    expect(parseColor("#FFF")).toEqual([255, 255, 255]);
    expect(parseColor("rgb(51, 51, 51)")).toEqual([51, 51, 51]);
  });

  it("returns null for transparent and invalid values", () => {
    expect(parseColor("none")).toBeNull();
    expect(parseColor("transparent")).toBeNull();
    expect(parseColor("url(#grad)")).toBeNull();
  });
});

describe("contrastRatio", () => {
  it("black and white contrast is 21, same color is 1", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 1);
    expect(contrastRatio("#4a3728", "#4a3728")).toBeCloseTo(1, 5);
  });

  it("the problematic color pair indeed has insufficient contrast", () => {
    // author's dark brown background + default theme's #333 label color — this is the cause of the black block.
    expect(contrastRatio("#4a3728", "#333333")).toBeLessThan(4.5);
  });
});

describe("legibleInkFor", () => {
  it("dark backgrounds get light text, light backgrounds get dark text", () => {
    expect(contrastRatio("#4a3728", legibleInkFor("#4a3728")!)).toBeGreaterThan(
      4.5,
    );
    expect(contrastRatio("#ECECFF", legibleInkFor("#ECECFF")!)).toBeGreaterThan(
      4.5,
    );
  });
});

describe("repairMermaidLabelContrast", () => {
  it("author hard-coded dark fill node: label is changed to readable light color", () => {
    const g = node({ shapeStyle: "fill:#4a3728 !important;stroke:#c9a227" });
    repairMermaidLabelContrast(mount(g));
    expectLegible(g, "#4a3728");
  });

  it("diamonds (polygons) are also fixed", () => {
    const g = node({ tag: "polygon", shapeStyle: "fill:#4a3728 !important" });
    repairMermaidLabelContrast(mount(g));
    expectLegible(g, "#4a3728");
  });

  it("subgraph cluster labels are also fixed", () => {
    const g = node({ cluster: true, shapeStyle: "fill:#22303c !important" });
    repairMermaidLabelContrast(mount(g));
    expectLegible(g, "#22303c");
  });

  it("light fill remains readable in dark theme (symmetric case)", () => {
    const g = node({ shapeStyle: "fill:#ffffff !important" });
    repairMermaidLabelContrast(mount(g));
    expectLegible(g, "#ffffff");
  });

  it("nodes without inline fill are never touched (theme colors are already coordinated)", () => {
    const g = node({});
    repairMermaidLabelContrast(mount(g));
    expect(g.querySelector("span")?.getAttribute("style")).toBeNull();
  });

  it("nodes where author explicitly set color are left unchanged", () => {
    const g = node({
      shapeStyle: "fill:#4a3728 !important",
      labelStyle: "color:#ffd166 !important",
    });
    repairMermaidLabelContrast(mount(g));
    expect(ink(g)).toBe("#ffd166");
  });

  it("idempotent: running twice gives same result and doesn't stack declarations", () => {
    const g = node({ shapeStyle: "fill:#4a3728 !important" });
    const svg = mount(g);
    repairMermaidLabelContrast(svg);
    const once = g.querySelector("span")?.getAttribute("style");
    repairMermaidLabelContrast(svg);
    expect(g.querySelector("span")?.getAttribute("style")).toBe(once);
  });
});

describe("repairMermaidContrastInSvg", () => {
  const svg = (labelStyle = "") =>
    `<svg xmlns="http://www.w3.org/2000/svg"><g class="node default">` +
    `<rect style="fill:#4a3728 !important"></rect>` +
    `<g class="label"><foreignObject><span class="nodeLabel"${labelStyle}>x</span>` +
    `</foreignObject></g></g></svg>`;

  it("bakes readable ink color into the string (so re-injection still results in fixed color)", () => {
    const out = repairMermaidContrastInSvg(svg());
    expect(out).toContain("color:#f5f5f5");
  });

  it("author-written colors are still untouched", () => {
    const out = repairMermaidContrastInSvg(svg(' style="color:#ffd166"'));
    expect(out).toContain("#ffd166");
    expect(out).not.toContain("#f5f5f5");
  });

  it("labels with <br> are also fixed (mermaid multiline labels look like this, not valid XML)", () => {
    const withBr =
      `<svg xmlns="http://www.w3.org/2000/svg"><g class="node default">` +
      `<rect style="fill:#4a3728 !important"></rect>` +
      `<g class="label"><foreignObject><span class="nodeLabel">` +
      `<p>④ Canonicalize<br>subject → LEI/FIGI</p></span></foreignObject></g></g></svg>`;
    expect(repairMermaidContrastInSvg(withBr)).toContain("color:#f5f5f5");
  });

  it("returns unchanged when there's no <svg>, doesn't lose content", () => {
    expect(repairMermaidContrastInSvg("mermaid 渲染失败了")).toBe(
      "mermaid 渲染失败了",
    );
  });

  it("incomplete svg is not lost (HTML parser fills in gaps, image is still there)", () => {
    const out = repairMermaidContrastInSvg("<svg><g class=\"node\"></g>");
    expect(out).toContain("<svg");
    expect(out).toContain("node");
  });
});
