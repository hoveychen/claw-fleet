import { describe, expect, it } from "vitest";
import { contrastRatio, parseColor } from "./mermaidContrast";
import {
  MERMAID_THEME_VARIABLES,
  type MermaidMode,
  mermaidThemeConfig,
} from "./mermaidTheme";

const MODES: MermaidMode[] = ["light", "dark"];

/** These keys' values are not colors (font size, font stack, toggles, line width), shouldn't be validated as hex. */
const NON_COLOR = /^(fontFamily|fontSize|darkMode|pie(Stroke|Outer)?Width|pieOpacity|pieStrokeWidth|pieOuterStrokeWidth)$/;

describe("mermaid theme variables", () => {
  it.each(MODES)("%s: all color values are opaque hex (khroma derives from them)", (mode) => {
    for (const [key, value] of Object.entries(MERMAID_THEME_VARIABLES[mode])) {
      if (NON_COLOR.test(key)) continue;
      expect(value, `${mode}.${key}`).toMatch(/^#[0-9a-f]{6}$/i);
      expect(parseColor(value), `${mode}.${key}`).not.toBeNull();
    }
  });

  it.each(MODES)("%s: node text on node background reaches AA (4.5:1)", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    expect(contrastRatio(v.mainBkg, v.nodeTextColor)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(v.actorBkg, v.actorTextColor)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(v.stateBkg, v.stateLabelColor)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(v.noteBkgColor, v.noteTextColor)).toBeGreaterThanOrEqual(4.5);
  });

  it.each(MODES)("%s: subgraph title on subgraph background reaches 3:1", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    expect(contrastRatio(v.clusterBkg, v.titleColor)).toBeGreaterThanOrEqual(3);
  });

  it.each(MODES)("%s: lines visible on canvas (3:1)", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    expect(contrastRatio(v.background, v.lineColor)).toBeGreaterThanOrEqual(3);
  });

  it.each(MODES)("%s: node border and background distinguishable (1.2:1)", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    // Node border just lifts the node off the page, carries no information, so threshold is much lower than text.
    expect(contrastRatio(v.mainBkg, v.nodeBorder)).toBeGreaterThanOrEqual(1.2);
  });

  it.each(MODES)("%s: category colors distinguish pairwise, paired ink color is readable", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    const scale = Array.from({ length: 8 }, (_, i) => v[`cScale${i}`]);
    expect(new Set(scale).size).toBe(scale.length);
    for (const fill of scale) {
      expect(contrastRatio(fill, v.scaleLabelColor)).toBeGreaterThanOrEqual(3);
    }
  });

  it("Light and dark are two different sets of values", () => {
    expect(MERMAID_THEME_VARIABLES.light.mainBkg).not.toBe(
      MERMAID_THEME_VARIABLES.dark.mainBkg,
    );
  });
});

describe("mermaidThemeConfig", () => {
  it.each(MODES)("%s: uses base theme, no longer uses mermaid built-in palette", (mode) => {
    const cfg = mermaidThemeConfig(mode);
    expect(cfg.theme).toBe("base");
    expect(cfg.themeVariables).toBe(MERMAID_THEME_VARIABLES[mode]);
  });

  it.each(MODES)("%s: font stack consistent in both places, else measure width and draw width mismatch", (mode) => {
    const cfg = mermaidThemeConfig(mode);
    expect(cfg.fontFamily).toBe("var(--font-sans)");
    expect(cfg.themeVariables.fontFamily).toBe(cfg.fontFamily);
  });

  it.each(MODES)("%s: themeCSS only rounds boxes where mermaid didn't write rx", (mode) => {
    expect(mermaidThemeConfig(mode).themeCSS).toContain(".node rect:not([rx])");
  });
});
