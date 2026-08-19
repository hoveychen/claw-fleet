import { describe, expect, it } from "vitest";

/**
 * 每个 `<ReactMarkdown>` 都必须显式传 `components`。
 *
 * 这条守的是本轮修掉的那类漂移：决策/计划 tab、工具详情、Fleet 工具结果三处
 * 各自 new 了一个 ReactMarkdown 却没传组件表，于是 ```mermaid fence 在手机上
 * 是一块原始代码，桌面却出图 —— 而 SessionDetailTabs 的注释还写着「同 wiki/
 * 消息视图」。漏传是静默的，只有人肉打开那一屏才看得见，所以在这里拦。
 *
 * 新增渲染面时：spread `mermaidMarkdownComponents`（要出图），或显式传一张
 * 不含它的表（确实只想要纯文本）—— 两种都过，唯独「忘了传」不过。
 */
// vite 的 glob：拿到 src 下每个 .tsx 的原文，不需要 node:fs（mobile-web 是纯浏览
// 器包，tsconfig 里没有 node 类型）。
const FILES = import.meta.glob("../**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

describe("mermaid 渲染面覆盖", () => {
  it("每个 <ReactMarkdown> 都传了 components", () => {
    const offenders: string[] = [];
    for (const [path, src] of Object.entries(FILES)) {
      if (path.endsWith(".test.tsx")) continue;
      // 每个开标签到它的 `>` 为止就是属性区。
      for (const m of src.matchAll(/<ReactMarkdown\b[\s\S]*?>/g)) {
        if (!m[0].includes("components=")) {
          offenders.push(`${path}:${src.slice(0, m.index).split("\n").length}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  // 决策卡那五处曾经只挂 remarkGfm，于是同一段文字在会话里 CJK 加粗、公式、软换行
  // 都对，进了决策卡就全不认。渲染面之间的差异应该只体现在组件表上，不该体现在
  // 插件链上。
  it("每个 <ReactMarkdown> 都用共享的插件链", () => {
    const offenders: string[] = [];
    for (const [path, src] of Object.entries(FILES)) {
      if (path.endsWith(".test.tsx")) continue;
      for (const m of src.matchAll(/<ReactMarkdown\b[\s\S]*?>/g)) {
        if (!m[0].includes("remarkPlugins={mdRemarkPlugins}")) {
          offenders.push(`${path}:${src.slice(0, m.index).split("\n").length}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});
