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
 * 复刻 mermaid 11 真实吐出的节点结构（用 probe 页在浏览器里 dump 过）：
 * 图形是 `g.node` 的直接子元素并带内联 fill，标签是 foreignObject 里的 span，
 * 颜色由 svg 内联样式表给（default 主题 `#333`），span 自己没有内联颜色。
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

/** 断言某个节点的标签色与它的底色对比度过关。 */
function expectLegible(g: Element, fill: string): void {
  const got = ink(g);
  expect(got, "标签没有被补上可读的颜色").not.toBeNull();
  expect(contrastRatio(fill, got!)).toBeGreaterThan(4.5);
}

describe("parseColor", () => {
  it("认 #rgb / #rrggbb / rgb() 三种写法", () => {
    expect(parseColor("#4a3728")).toEqual([74, 55, 40]);
    expect(parseColor("#FFF")).toEqual([255, 255, 255]);
    expect(parseColor("rgb(51, 51, 51)")).toEqual([51, 51, 51]);
  });

  it("透明和垃圾值返回 null", () => {
    expect(parseColor("none")).toBeNull();
    expect(parseColor("transparent")).toBeNull();
    expect(parseColor("url(#grad)")).toBeNull();
  });
});

describe("contrastRatio", () => {
  it("黑白是 21，同色是 1", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 1);
    expect(contrastRatio("#4a3728", "#4a3728")).toBeCloseTo(1, 5);
  });

  it("报出问题里那对配色确实不可读", () => {
    // 作者的深棕底 + default 主题的 #333 标签色 —— 这就是黑块的成因。
    expect(contrastRatio("#4a3728", "#333333")).toBeLessThan(4.5);
  });
});

describe("legibleInkFor", () => {
  it("深底给浅字、浅底给深字", () => {
    expect(contrastRatio("#4a3728", legibleInkFor("#4a3728")!)).toBeGreaterThan(
      4.5,
    );
    expect(contrastRatio("#ECECFF", legibleInkFor("#ECECFF")!)).toBeGreaterThan(
      4.5,
    );
  });
});

describe("repairMermaidLabelContrast", () => {
  it("作者硬编码深色 fill 的节点：标签改成能读的浅色", () => {
    const g = node({ shapeStyle: "fill:#4a3728 !important;stroke:#c9a227" });
    repairMermaidLabelContrast(mount(g));
    expectLegible(g, "#4a3728");
  });

  it("菱形（polygon）同样修", () => {
    const g = node({ tag: "polygon", shapeStyle: "fill:#4a3728 !important" });
    repairMermaidLabelContrast(mount(g));
    expectLegible(g, "#4a3728");
  });

  it("subgraph 的 cluster 标签也修", () => {
    const g = node({ cluster: true, shapeStyle: "fill:#22303c !important" });
    repairMermaidLabelContrast(mount(g));
    expectLegible(g, "#22303c");
  });

  it("浅色 fill 在深色主题下同样能读（对称情形）", () => {
    const g = node({ shapeStyle: "fill:#ffffff !important" });
    repairMermaidLabelContrast(mount(g));
    expectLegible(g, "#ffffff");
  });

  it("没有内联 fill 的节点一律不碰（主题配色本来就是配套的）", () => {
    const g = node({});
    repairMermaidLabelContrast(mount(g));
    expect(g.querySelector("span")?.getAttribute("style")).toBeNull();
  });

  it("作者显式写了 color 的节点保持原样", () => {
    const g = node({
      shapeStyle: "fill:#4a3728 !important",
      labelStyle: "color:#ffd166 !important",
    });
    repairMermaidLabelContrast(mount(g));
    expect(ink(g)).toBe("#ffd166");
  });

  it("幂等：跑两遍结果一样，且不堆叠声明", () => {
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

  it("把可读的墨色烤进字符串里（这样谁再注入一次都还是修好的）", () => {
    const out = repairMermaidContrastInSvg(svg());
    expect(out).toContain("color:#f5f5f5");
  });

  it("作者写了 color 的照旧不碰", () => {
    const out = repairMermaidContrastInSvg(svg(' style="color:#ffd166"'));
    expect(out).toContain("#ffd166");
    expect(out).not.toContain("#f5f5f5");
  });

  it("标签里带 <br> 也照修（mermaid 的多行标签就是这样，不是合法 XML）", () => {
    const withBr =
      `<svg xmlns="http://www.w3.org/2000/svg"><g class="node default">` +
      `<rect style="fill:#4a3728 !important"></rect>` +
      `<g class="label"><foreignObject><span class="nodeLabel">` +
      `<p>④ Canonicalize<br>subject → LEI/FIGI</p></span></foreignObject></g></g></svg>`;
    expect(repairMermaidContrastInSvg(withBr)).toContain("color:#f5f5f5");
  });

  it("里面根本没有 <svg> 时原样退回，不把内容弄丢", () => {
    expect(repairMermaidContrastInSvg("mermaid 渲染失败了")).toBe(
      "mermaid 渲染失败了",
    );
  });

  it("残缺的 svg 不会被丢掉（HTML 解析会补全，但图还在）", () => {
    const out = repairMermaidContrastInSvg("<svg><g class=\"node\"></g>");
    expect(out).toContain("<svg");
    expect(out).toContain("node");
  });
});
