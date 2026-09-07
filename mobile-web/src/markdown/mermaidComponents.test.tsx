import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import { mermaidMarkdownComponents } from "./mermaidComponents";
import { MD_BLOCK, MD_INLINE } from "../views/SessionDetailTabs";

// MermaidBlock 在 render 期读 html[data-theme] 挑主题。renderToStaticMarkup 不跑
// effect，所以这一个读取就是它对 DOM 的全部需求 —— 用一行 stub 顶掉，不必为此
// 给 mobile-web 引入 jsdom（桌面那侧才装了）。
vi.stubGlobal("document", { documentElement: { getAttribute: () => "light" } });

const MERMAID = "```mermaid\nflowchart TB\n  A --> B\n```";
const TS = "```ts\nconst a = 1;\n```";

function render(md: string, components: Components): string {
  return renderToStaticMarkup(
    <ReactMarkdown components={components}>{md}</ReactMarkdown>,
  );
}

/** 图渲染是异步的（mermaid 是懒加载的），所以服务端首帧只有空的容器 div；
 *  判据是「fence 没有被当成代码块吐出来」，而不是有没有 <svg>。 */
function rendersDiagram(html: string): boolean {
  return !html.includes("language-mermaid") && !html.includes("flowchart TB");
}

describe("mermaidMarkdownComponents", () => {
  it("mermaid fence 换成图容器，不再是代码块", () => {
    expect(rendersDiagram(render(MERMAID, mermaidMarkdownComponents))).toBe(true);
  });

  it("普通 fence 仍是带 <pre> 的代码块", () => {
    const html = render(TS, mermaidMarkdownComponents);
    expect(html).toContain("<pre");
    expect(html).toContain("const a = 1;");
  });
});

// 决策/计划 tab 曾经漏接 mermaid（组件表只盖了 a），注释却写着「同 wiki/消息
// 视图」。这两条把它钉住。
describe("SessionDetailTabs 的决策正文组件表", () => {
  it("MD_BLOCK 认 mermaid", () => {
    expect(rendersDiagram(render(MERMAID, MD_BLOCK))).toBe(true);
  });

  it("MD_INLINE 也认（选项标签里也可能带图）", () => {
    expect(rendersDiagram(render(MERMAID, MD_INLINE))).toBe(true);
  });

  // 曾经这里断言链接必须是 inert 的 <span>：外链在手机上点不开的那条 bug 就
  // 长在这个断言底下。现在外链一律交给系统浏览器（壳里由 launchIntent /
  // onLoadIntercept 接管），认不出的 scheme 才继续不可点。
  it("外链是真 <a>，未知 scheme 仍不可点", () => {
    expect(render("[x](https://example.com)", MD_BLOCK)).toContain("<a ");
    expect(render("[x](./a.md)", MD_BLOCK)).not.toContain("href=");
  });
});
