/**
 * Mermaid's color palette, wired to the App.css design tokens.
 *
 * Previously, this passed mermaid's built-in themes directly (light → `default`,
 * dark → `dark`), which gave you mermaid's factory palette: ginger-yellow
 * subgraphs (#ffffde), pale purple nodes (#ECECFF), violet strokes (#9370DB),
 * 16px trebuchet. The diagram was the only place in the entire app that didn't
 * use its own tokens.
 *
 * After switching to `theme: "base"`, mermaid treats every variable supplied
 * here as authoritative (`Theme.calculate` applies overrides, then derives, then
 * applies overrides again, so explicit keys always win), and only omitted keys
 * are derived from primaryColor.
 *
 * **Values must be opaque hex.** Mermaid uses khroma to derive darken/lighten/
 * invert from these colors; half-transparent colors derive unpredictably, and
 * CSS variables like `var(--x)` fail to parse in khroma entirely. So this table
 * is a hand-flattened copy of App.css's token values into hex — when you change
 * App.css's base/text colors, sync them here (mermaidTheme.test.ts validates
 * contrast only, not sync). The only exception is fontFamily: it's a CSS string
 * and doesn't go through khroma.
 *
 * Desktop and mobile each have their own copy (same convention as
 * mermaidContrast.ts); change one, change both.
 */

/** Mermaid measures text width with the diagram hanging from document.body, but
 *  when rendered it may land inside markdown's <pre> and inherit a monospaced
 *  font — a label measured at 92px renders at 116px and gets clipped by the
 *  node box. Give it a CSS variable that resolves to the same stack in both
 *  places so measurements and rendering align. Can't use "inherit". */
const FONT_FAMILY = "var(--font-sans)";

/** Categorical colors: for pie / journey / timeline / gitGraph charts that
 *  "distinguish items by color." Without these, they derive a bunch of
 *  indistinguishable grays from the nearly-neutral primaryColor. */
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

/** Spread categorical colors across the two sets of keys mermaid expects:
 *  cScale0..7 and pie1..8. */
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

  // ── Canvas and base typography ──
  background: "#f1efea", // --color-bg
  fontFamily: FONT_FAMILY,
  fontSize: "14px",

  // ── Nodes (flowchart / class / state share mainBkg + nodeBorder) ──
  primaryColor: "#fbfaf7", // --color-bg-card
  mainBkg: "#fbfaf7",
  nodeBkg: "#fbfaf7",
  nodeBorder: "#c9c5bb",
  primaryBorderColor: "#c9c5bb",
  primaryTextColor: "#1f2023", // --color-text
  textColor: "#1f2023",
  nodeTextColor: "#1f2023",
  classText: "#1f2023",

  // ── Connectors ──
  lineColor: "#88837a",
  arrowheadColor: "#88837a",
  defaultLinkColor: "#88837a",
  edgeLabelBackground: "#f1efea",

  // ── Subgraph / cluster: one level back from body text, title uses secondary text color ──
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

  // ── Note: the only place that keeps warm yellow, because notes should stand out ──
  noteBkgColor: "#f7f0dd", // --color-warning-bg
  noteTextColor: "#4a4436",
  noteBorderColor: "#e0d5b4",

  // ── Sequence diagram ──
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

  // ── State diagram ──
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

  // ── ER diagram ──
  attributeBackgroundColorOdd: "#fbfaf7",
  attributeBackgroundColorEven: "#f3f1ec",
  rowOdd: "#fbfaf7",
  rowEven: "#f3f1ec",

  // ── Gantt ──
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
 * Geometric details the mermaid variable table can't reach, patched with a
 * little CSS.
 *
 * In SVG2, `rx`/`ry` are CSS geometric properties supported by Chromium and
 * WebKit, so they can directly turn square boxes into rounded corners. The
 * `:not([rx])` selector ensures we only modify shapes where mermaid didn't set
 * rx — when an author writes `A(圆角)`, mermaid sets rx as an attribute, which
 * is their choice and we don't override it.
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

/** Get the complete theme config to pass to `mermaid.initialize` in one call. */
export function mermaidThemeConfig(mode: MermaidMode) {
  return {
    theme: "base" as const,
    themeVariables: MERMAID_THEME_VARIABLES[mode],
    themeCSS: mermaidThemeCss(mode),
    fontFamily: FONT_FAMILY,
  };
}
