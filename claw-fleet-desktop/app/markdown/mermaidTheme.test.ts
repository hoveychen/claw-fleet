import { describe, expect, it } from "vitest";
import { contrastRatio, parseColor } from "./mermaidContrast";
import {
  MERMAID_THEME_VARIABLES,
  type MermaidMode,
  mermaidThemeConfig,
} from "./mermaidTheme";

const MODES: MermaidMode[] = ["light", "dark"];

/** 这些 key 的值不是颜色（字号、字体栈、开关、线宽），不该按 hex 校验。 */
const NON_COLOR = /^(fontFamily|fontSize|darkMode|pie(Stroke|Outer)?Width|pieOpacity|pieStrokeWidth|pieOuterStrokeWidth)$/;

describe("mermaid 主题变量", () => {
  it.each(MODES)("%s：颜色值全是不透明 hex（khroma 要拿去派生）", (mode) => {
    for (const [key, value] of Object.entries(MERMAID_THEME_VARIABLES[mode])) {
      if (NON_COLOR.test(key)) continue;
      expect(value, `${mode}.${key}`).toMatch(/^#[0-9a-f]{6}$/i);
      expect(parseColor(value), `${mode}.${key}`).not.toBeNull();
    }
  });

  it.each(MODES)("%s：节点文字压在节点底色上达到 AA（4.5:1）", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    expect(contrastRatio(v.mainBkg, v.nodeTextColor)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(v.actorBkg, v.actorTextColor)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(v.stateBkg, v.stateLabelColor)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(v.noteBkgColor, v.noteTextColor)).toBeGreaterThanOrEqual(4.5);
  });

  it.each(MODES)("%s：subgraph 标题压在 subgraph 底色上达到 3:1", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    expect(contrastRatio(v.clusterBkg, v.titleColor)).toBeGreaterThanOrEqual(3);
  });

  it.each(MODES)("%s：连线在画布上看得见（3:1）", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    expect(contrastRatio(v.background, v.lineColor)).toBeGreaterThanOrEqual(3);
  });

  it.each(MODES)("%s：节点描边和节点底色分得开（1.2:1）", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    // 描边只是把节点从纸面上托起来，不承载信息，所以门槛比文字低得多。
    expect(contrastRatio(v.mainBkg, v.nodeBorder)).toBeGreaterThanOrEqual(1.2);
  });

  it.each(MODES)("%s：分类色两两可分，且配的墨色读得出来", (mode) => {
    const v = MERMAID_THEME_VARIABLES[mode];
    const scale = Array.from({ length: 8 }, (_, i) => v[`cScale${i}`]);
    expect(new Set(scale).size).toBe(scale.length);
    for (const fill of scale) {
      expect(contrastRatio(fill, v.scaleLabelColor)).toBeGreaterThanOrEqual(3);
    }
  });

  it("亮色和深色是两套不同的值", () => {
    expect(MERMAID_THEME_VARIABLES.light.mainBkg).not.toBe(
      MERMAID_THEME_VARIABLES.dark.mainBkg,
    );
  });
});

describe("mermaidThemeConfig", () => {
  it.each(MODES)("%s：走 base 主题，不再用 mermaid 内置色板", (mode) => {
    const cfg = mermaidThemeConfig(mode);
    expect(cfg.theme).toBe("base");
    expect(cfg.themeVariables).toBe(MERMAID_THEME_VARIABLES[mode]);
  });

  it.each(MODES)("%s：字体栈两处一致，否则量宽和画宽对不上", (mode) => {
    const cfg = mermaidThemeConfig(mode);
    expect(cfg.fontFamily).toBe("var(--font-sans)");
    expect(cfg.themeVariables.fontFamily).toBe(cfg.fontFamily);
  });

  it.each(MODES)("%s：themeCSS 只圆化 mermaid 自己没写 rx 的方框", (mode) => {
    expect(mermaidThemeConfig(mode).themeCSS).toContain(".node rect:not([rx])");
  });
});
