/**
 * 修 mermaid 图里「作者硬编码了 fill、标签颜色却还是主题默认色」导致的黑块。
 *
 * mermaid 把 `style X fill:#4a3728` 写成节点图形上的内联 `fill`，但标签颜色来自
 * 主题内联样式表（default 主题是 `#333`，dark 主题是浅色）。于是一张按深色主题
 * 配色写的图，在 light 主题下就是深字压深底 —— 整个节点糊成一个黑块。
 *
 * 这里在渲染完成后补一刀：凡是图形带了作者指定 fill、而标签自己没指定颜色的
 * 节点/子图，按 fill 的亮度把标签改成黑或白。没指定 fill 的节点不碰（主题默认
 * 配色本来就是对的），作者显式写了 `color:` 的也不碰（那是他自己的选择）。
 *
 * 与桌面 claw-fleet-desktop/app/markdown/mermaidContrast.ts 保持同步（两个 app 是
 * 独立的 vite 包，逻辑复制而非共享）；单元测试在桌面那一侧。
 */

/** 补色候选：够黑但不是纯黑，够白但不刺眼。 */
const LIGHT_INK = "#f5f5f5";
const DARK_INK = "#1a1a1a";

type Rgb = [number, number, number];

/** 只解析 mermaid 实际会吐出来的两种写法：`#rgb`/`#rrggbb` 与 `rgb()/rgba()`。 */
export function parseColor(css: string): Rgb | null {
  const s = css.trim().toLowerCase();
  if (s === "" || s === "none" || s === "transparent") return null;
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/.exec(s);
  if (hex) {
    const h = hex[1];
    const wide =
      h.length === 3
        ? h
            .split("")
            .map((c) => c + c)
            .join("")
        : h;
    return [
      parseInt(wide.slice(0, 2), 16),
      parseInt(wide.slice(2, 4), 16),
      parseInt(wide.slice(4, 6), 16),
    ];
  }
  const fn = /^rgba?\(([^)]+)\)$/.exec(s);
  if (fn) {
    const parts = fn[1].split(/[\s,/]+/).filter((p) => p !== "");
    if (parts.length < 3) return null;
    const nums = parts.slice(0, 3).map((p) => Number.parseFloat(p));
    if (nums.some((n) => Number.isNaN(n))) return null;
    return [nums[0], nums[1], nums[2]] as Rgb;
  }
  return null;
}

/** WCAG 相对亮度。 */
function luminance([r, g, b]: Rgb): number {
  const lin = [r, g, b].map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
}

/** WCAG 对比度，1（同色）到 21（黑白）。 */
export function contrastRatio(a: string, b: string): number {
  const ca = parseColor(a);
  const cb = parseColor(b);
  if (!ca || !cb) return 1;
  const la = luminance(ca);
  const lb = luminance(cb);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** 给定底色，选对比度更高的墨色；底色解析不了就返回 null（不动它）。 */
export function legibleInkFor(fill: string): string | null {
  if (!parseColor(fill)) return null;
  return contrastRatio(fill, LIGHT_INK) >= contrastRatio(fill, DARK_INK)
    ? LIGHT_INK
    : DARK_INK;
}

/** 从 `style` 属性里取某个声明的值，顺手剥掉 `!important`。 */
function declaration(el: Element, prop: string): string | null {
  const raw = el.getAttribute("style");
  if (!raw) return null;
  for (const decl of raw.split(";")) {
    const at = decl.indexOf(":");
    if (at < 0) continue;
    if (decl.slice(0, at).trim().toLowerCase() !== prop) continue;
    return decl
      .slice(at + 1)
      .replace(/!important/i, "")
      .trim();
  }
  return null;
}

const SHAPE = [
  ":scope > rect",
  ":scope > polygon",
  ":scope > circle",
  ":scope > ellipse",
  ":scope > path",
].join(",");

const LABEL = [
  ":scope > g.label span",
  ":scope > g.label p",
  ":scope > g.label text",
  ":scope > g.label tspan",
  ":scope > g.cluster-label span",
  ":scope > g.cluster-label p",
  ":scope > g.cluster-label text",
  ":scope > g.cluster-label tspan",
].join(",");

/**
 * 就地修一棵已渲染的 mermaid SVG（root 是包着 `<svg>` 的容器或 svg 本身）。
 * 幂等：重复跑只会把同样的颜色再写一遍。
 */
export function repairMermaidLabelContrast(root: ParentNode): void {
  for (const group of Array.from(root.querySelectorAll("g.node, g.cluster"))) {
    const shape = group.querySelector(SHAPE);
    if (!shape) continue;
    const fill = declaration(shape, "fill");
    // 没有作者指定的 fill —— 主题自带的配色和标签色本来就是配套的，别插手。
    if (!fill) continue;
    const ink = legibleInkFor(fill);
    if (!ink) continue;
    for (const label of Array.from(group.querySelectorAll(LABEL))) {
      // 作者自己写了 `color:`，mermaid 会把它内联到标签上——那是他的选择，
      // 只有我们上一轮补的那两支墨色才允许被再写一遍（保证幂等）。
      const own = declaration(label, "color") ?? declaration(label, "fill");
      if (own !== null && own !== LIGHT_INK && own !== DARK_INK) continue;
      const prior = label.getAttribute("style") ?? "";
      const kept = prior
        .split(";")
        .filter((d) => {
          const name = d.slice(0, d.indexOf(":")).trim().toLowerCase();
          return d.trim() !== "" && name !== "color" && name !== "fill";
        })
        .join(";");
      const patch = `color:${ink} !important;fill:${ink} !important`;
      label.setAttribute("style", kept === "" ? patch : `${kept};${patch}`);
    }
  }
}

/**
 * 同样的修复，但作用在 mermaid 吐出的 SVG **字符串**上。
 *
 * 挂载后再改 DOM 是脆的：决策卡那条路径实测会在效果跑完之后重新注入一次同一
 * 段 SVG，把补好的标签色整片冲掉（`data-repair` 属性还在、里面的 span 却又变
 * 回没样式）。把颜色烤进字符串里，谁再注入一次都还是修好的那份。
 */
export function repairMermaidContrastInSvg(svgText: string): string {
  if (typeof document === "undefined") return svgText;
  // 用 HTML 解析器而不是 XML 解析器：mermaid 的多行标签里是裸 `<br>`，
  // `DOMParser(..., "image/svg+xml")` 会直接判成 parsererror 整段放弃修复
  // （实测桌面 wiki 那张图就是这样漏掉的）。innerHTML 走的是宽容的 HTML 路径，
  // 和 React 注入这段 SVG 时用的是同一套解析。
  const host = document.createElement("div");
  host.innerHTML = svgText;
  const svg = host.querySelector("svg");
  if (!svg) return svgText;
  repairMermaidLabelContrast(svg);
  return host.innerHTML;
}
