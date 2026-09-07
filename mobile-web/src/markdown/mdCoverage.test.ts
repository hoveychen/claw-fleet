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

describe("markdown 渲染面覆盖", () => {
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
  // 消息页把 `a` 覆写成一个 `<span className={styles.mdLink}>`：看着是链接、点
  // 下去什么都不发生，从会话详情页第一版起一直如此，只有人肉在手机上点一下才
  // 看得见。唯一合法的 inert 链接是折叠带的标题（它整块在一个 <button> 里），
  // 它显式列在白名单里。
  const INERT_LINK_OK = new Set(["bandTitleMdComponents"]);
  it("没有渲染面把链接做成不可点的 span", () => {
    const offenders: string[] = [];
    for (const [path, src] of Object.entries(FILES)) {
      if (path.endsWith(".test.tsx")) continue;
      // 两种写法都拦：组件表里内联的 `a: (…) => <span>`，和先单独声明一个
      // `const x: Components["a"] = (…) => <span>` 再挂上去（DecisionQa 就是后
      // 者，所以第一版守门没看见它）。
      const INERT = /(?:a: |Components\["a"\] = )\(\{[^)]*\}[^)]*\) => \(?\s*<span/g;
      for (const m of src.matchAll(INERT)) {
        const line = src.slice(0, m.index).split("\n").length;
        // 白名单按「这张组件表的变量名」判定：往上找最近的 `const X = {`。
        const decl = [...src.slice(0, m.index).matchAll(/const (\w+)[^=]*= /g)].pop();
        if (decl && INERT_LINK_OK.has(decl[1])) continue;
        offenders.push(`${path}:${line}`);
      }
    }
    expect(offenders).toEqual([]);
  });

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
