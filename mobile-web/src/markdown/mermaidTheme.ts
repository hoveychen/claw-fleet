/**
 * mermaid 的配色，接到 App.css 的设计 token 上。
 *
 * 之前这里直接把 mermaid 的内置主题喂进去（light → `default`，dark → `dark`），
 * 拿到的是 mermaid 出厂色板：姜黄 subgraph（#ffffde）、淡紫节点（#ECECFF）、
 * 紫罗兰描边（#9370DB）、16px trebuchet。全应用只有图表这一处不吃自己的 token。
 *
 * 改成 `theme: "base"` 之后，mermaid 会把这里给的每个变量当权威值（`Theme.calculate`
 * 先应用一遍 overrides、跑完派生、再应用一遍 overrides，所以显式写的 key 一定生效），
 * 没写的 key 才从 primaryColor 派生。
 *
 * **值必须是不透明 hex。** mermaid 用 khroma 对这些颜色做 darken/lighten/invert 派生，
 * 半透明色派生出来的结果不可控；而且 `var(--x)` 这类 CSS 变量在 khroma 里直接解析失败。
 * 所以这张表是把 App.css 的 token 值**手工压平**成 hex 的一份副本 —— 改 App.css 的
 * 底色/文字色时，这里要跟着改（mermaidTheme.test.ts 只校验对比度，不校验同步）。
 * 唯一的例外是 fontFamily：那是 CSS 字符串，不进 khroma。
 *
 * 桌面端和移动端各有一份（和 mermaidContrast.ts 同样的约定），改一处要改两处。
 */

/** mermaid 量文字宽度时把图挂在 document.body 下，画的时候却可能落在 markdown 的
 *  <pre> 里继承到等宽字体 —— 量出来 92px 的标签实际画 116px，直接被节点框切掉。
 *  给一个两处都解析成同一个栈的 CSS 变量，量和画就对得上。不能用 "inherit"。 */
const FONT_FAMILY = "var(--font-sans)";

/** 分类色：给 pie / journey / timeline / gitGraph 这些「靠颜色区分条目」的图。
 *  不给的话它们会从近乎中性的 primaryColor 派生出一堆分不开的灰。 */
const CATEGORICAL_LIGHT = [
  "#c25232", // accent
  "#1d4ed8", // accent-tool
  "#1b7f87", // teal
  "#178042", // success
  "#9a6a08", // warning
  "#8250df", // info
  "#b3442a", // accent-bright
  "#5d6168", // text-secondary
];

const CATEGORICAL_DARK = [
  "#d97757",
  "#7dd3fc",
  "#39c5cf",
  "#4ade80",
  "#fbbf24",
  "#a371f7",
  "#f0a070",
  "#8a8f98",
];

export type MermaidMode = "light" | "dark";

/** 把分类色摊成 mermaid 要的 cScale0..7 / pie1..8 两组 key。 */
function scaleKeys(scale: string[], labelInk: string): Record<string, string> {
  const out: Record<string, string> = { scaleLabelColor: labelInk };
  scale.forEach((color, i) => {
    out[`cScale${i}`] = color;
    out[`cScaleLabel${i}`] = labelInk;
    out[`pie${i + 1}`] = color;
  });
  return out;
}

const LIGHT: Record<string, string> = {
  darkMode: "false",

  // —— 画布与基础排版 ——
  background: "#f1efea", // --color-bg
  fontFamily: FONT_FAMILY,
  fontSize: "14px",

  // —— 节点（flowchart / class / state 共用 mainBkg + nodeBorder）——
  primaryColor: "#fbfaf7", // --color-bg-card
  mainBkg: "#fbfaf7",
  nodeBkg: "#fbfaf7",
  nodeBorder: "#c9c5bb",
  primaryBorderColor: "#c9c5bb",
  primaryTextColor: "#1f2023", // --color-text
  textColor: "#1f2023",
  nodeTextColor: "#1f2023",
  classText: "#1f2023",

  // —— 连线 ——
  lineColor: "#88837a",
  arrowheadColor: "#88837a",
  defaultLinkColor: "#88837a",
  edgeLabelBackground: "#f1efea",

  // —— subgraph / cluster：比正文纸面沉一档，标题走次级文字色 ——
  clusterBkg: "#eae7e0",
  clusterBorder: "#d8d4ca",
  titleColor: "#6f7078", // --color-text-dim

  secondaryColor: "#efece5",
  secondaryBorderColor: "#d8d4ca",
  secondaryTextColor: "#1f2023",
  tertiaryColor: "#eae7e0",
  tertiaryBorderColor: "#d8d4ca",
  tertiaryTextColor: "#1f2023",
  border2: "#d8d4ca",

  // —— note：唯一保留暖黄的地方，因为 note 本来就该跳出来 ——
  noteBkgColor: "#f7f0dd", // --color-warning-bg
  noteTextColor: "#4a4436",
  noteBorderColor: "#e0d5b4",

  // —— sequenceDiagram ——
  actorBkg: "#fbfaf7",
  actorBorder: "#c9c5bb",
  actorTextColor: "#1f2023",
  actorLineColor: "#c9c5bb",
  signalColor: "#5d6168",
  signalTextColor: "#5d6168",
  labelBoxBkgColor: "#eae7e0",
  labelBoxBorderColor: "#d8d4ca",
  labelTextColor: "#1f2023",
  loopTextColor: "#5d6168",
  activationBkgColor: "#e7e4dd",
  activationBorderColor: "#c9c5bb",
  sequenceNumberColor: "#fbfaf7",

  // —— stateDiagram ——
  stateBkg: "#fbfaf7",
  stateLabelColor: "#1f2023",
  labelBackgroundColor: "#f1efea",
  compositeBackground: "#eae7e0",
  compositeTitleBackground: "#e7e4dd",
  compositeBorder: "#d8d4ca",
  altBackground: "#eae7e0",
  transitionColor: "#88837a",
  transitionLabelColor: "#5d6168",
  specialStateColor: "#1f2023",

  // —— erDiagram ——
  attributeBackgroundColorOdd: "#fbfaf7",
  attributeBackgroundColorEven: "#f3f1ec",
  rowOdd: "#fbfaf7",
  rowEven: "#f3f1ec",

  // —— gantt ——
  sectionBkgColor: "#eae7e0",
  sectionBkgColor2: "#f3f1ec",
  altSectionBkgColor: "#f1efea",
  taskBkgColor: "#e3ded3",
  taskBorderColor: "#c9c5bb",
  taskTextColor: "#1f2023",
  taskTextDarkColor: "#1f2023",
  taskTextLightColor: "#fbfaf7",
  taskTextOutsideColor: "#5d6168",
  activeTaskBkgColor: "#c25232",
  activeTaskBorderColor: "#a94527",
  doneTaskBkgColor: "#d8d4ca",
  doneTaskBorderColor: "#b8b3a8",
  critBkgColor: "#cc3340",
  critBorderColor: "#a82733",
  gridColor: "#ddd9d0",
  todayLineColor: "#c25232",

  ...scaleKeys(CATEGORICAL_LIGHT, "#ffffff"),
  pieStrokeColor: "#f1efea",
  pieOuterStrokeColor: "#d8d4ca",
  pieStrokeWidth: "1px",
  pieOuterStrokeWidth: "1px",
  pieOpacity: "1",
  pieTitleTextColor: "#1f2023",
  pieSectionTextColor: "#ffffff",
  pieLegendTextColor: "#1f2023",
};

const DARK: Record<string, string> = {
  darkMode: "true",

  background: "#0f1011",
  fontFamily: FONT_FAMILY,
  fontSize: "14px",

  primaryColor: "#1c1e21", // --color-bg-card
  mainBkg: "#1c1e21",
  nodeBkg: "#1c1e21",
  nodeBorder: "#3a3d42",
  primaryBorderColor: "#3a3d42",
  primaryTextColor: "#f7f8f8", // --color-text
  textColor: "#f7f8f8",
  nodeTextColor: "#f7f8f8",
  classText: "#f7f8f8",

  lineColor: "#6b7078",
  arrowheadColor: "#6b7078",
  defaultLinkColor: "#6b7078",
  edgeLabelBackground: "#0f1011",

  clusterBkg: "#17191b",
  clusterBorder: "#2b2e32",
  titleColor: "#8a8f98", // --color-text-secondary

  secondaryColor: "#232528",
  secondaryBorderColor: "#2b2e32",
  secondaryTextColor: "#f7f8f8",
  tertiaryColor: "#191b1e",
  tertiaryBorderColor: "#2b2e32",
  tertiaryTextColor: "#f7f8f8",
  border2: "#2b2e32",

  noteBkgColor: "#33290a", // --color-warning-bg
  noteTextColor: "#f0e6c8",
  noteBorderColor: "#5a4a1a",

  actorBkg: "#1c1e21",
  actorBorder: "#3a3d42",
  actorTextColor: "#f7f8f8",
  actorLineColor: "#3a3d42",
  signalColor: "#8a8f98",
  signalTextColor: "#8a8f98",
  labelBoxBkgColor: "#232528",
  labelBoxBorderColor: "#3a3d42",
  labelTextColor: "#f7f8f8",
  loopTextColor: "#8a8f98",
  activationBkgColor: "#292b2f",
  activationBorderColor: "#3a3d42",
  sequenceNumberColor: "#0f1011",

  stateBkg: "#1c1e21",
  stateLabelColor: "#f7f8f8",
  labelBackgroundColor: "#0f1011",
  compositeBackground: "#17191b",
  compositeTitleBackground: "#232528",
  compositeBorder: "#2b2e32",
  altBackground: "#17191b",
  transitionColor: "#6b7078",
  transitionLabelColor: "#8a8f98",
  specialStateColor: "#f7f8f8",

  attributeBackgroundColorOdd: "#1c1e21",
  attributeBackgroundColorEven: "#191b1e",
  rowOdd: "#1c1e21",
  rowEven: "#191b1e",

  sectionBkgColor: "#17191b",
  sectionBkgColor2: "#1c1e21",
  altSectionBkgColor: "#0f1011",
  taskBkgColor: "#292b2f",
  taskBorderColor: "#3a3d42",
  taskTextColor: "#f7f8f8",
  taskTextDarkColor: "#0f1011",
  taskTextLightColor: "#f7f8f8",
  taskTextOutsideColor: "#8a8f98",
  activeTaskBkgColor: "#d97757",
  activeTaskBorderColor: "#f0a070",
  doneTaskBkgColor: "#2b2e32",
  doneTaskBorderColor: "#3a3d42",
  critBkgColor: "#f87171",
  critBorderColor: "#dc2626",
  gridColor: "#2b2e32",
  todayLineColor: "#d97757",

  ...scaleKeys(CATEGORICAL_DARK, "#0f1011"),
  pieStrokeColor: "#0f1011",
  pieOuterStrokeColor: "#2b2e32",
  pieStrokeWidth: "1px",
  pieOuterStrokeWidth: "1px",
  pieOpacity: "1",
  pieTitleTextColor: "#f7f8f8",
  pieSectionTextColor: "#0f1011",
  pieLegendTextColor: "#f7f8f8",
};

export const MERMAID_THEME_VARIABLES: Record<MermaidMode, Record<string, string>> = {
  light: LIGHT,
  dark: DARK,
};

/**
 * mermaid 的变量表管不到的几何细节，用一小段 CSS 补。
 *
 * `rx`/`ry` 在 SVG2 里是 CSS 几何属性，Chromium 和 WebKit 都支持，所以能直接把
 * 直角方框改圆角。`:not([rx])` 是为了只动 mermaid 没写 rx 的形状 —— 作者写
 * `A(圆角)` 时 mermaid 会把 rx 落成属性，那是他的选择，不覆盖。
 */
export function mermaidThemeCss(mode: MermaidMode): string {
  const shadow =
    mode === "light"
      ? "drop-shadow(0 1px 1.5px rgba(32, 28, 18, 0.10))"
      : "drop-shadow(0 1px 2px rgba(0, 0, 0, 0.45))";
  return `
    .node rect:not([rx]) { rx: 8px; ry: 8px; }
    .node rect, .node circle, .node ellipse, .node polygon, .node path {
      filter: ${shadow};
    }
    .cluster rect { rx: 12px; ry: 12px; }
    .cluster-label, .cluster span, .cluster-label foreignObject div {
      font-size: 12.5px;
      font-weight: 500;
      letter-spacing: 0.01em;
    }
    .edgeLabel, .edgeLabel span, .edgeLabel p { font-size: 12.5px; }
  `;
}

/** 一次调用拿到喂给 `mermaid.initialize` 的整段主题配置。 */
export function mermaidThemeConfig(mode: MermaidMode) {
  return {
    theme: "base" as const,
    themeVariables: MERMAID_THEME_VARIABLES[mode],
    themeCSS: mermaidThemeCss(mode),
    fontFamily: FONT_FAMILY,
  };
}
