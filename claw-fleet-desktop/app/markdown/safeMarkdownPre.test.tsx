// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";

// safeLinks reaches for Tauri's opener at import time; the components under
// test never call it.
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { safeMarkdownComponents } = await import("./safeLinks");

function render(md: string): string {
  return renderToStaticMarkup(
    <ReactMarkdown components={safeMarkdownComponents}>{md}</ReactMarkdown>,
  );
}

describe("safeMarkdownComponents 的 pre 覆写", () => {
  it("mermaid fence 不再被外层 <pre> 包住", () => {
    // <pre> 会把图裹进等宽字体的框里 —— mermaid 是按 sans 量的标签宽度，继承到
    // 等宽后就画不下、被节点框切掉。
    const html = render("```mermaid\nflowchart TB\n  A --> B\n```");
    expect(html).not.toContain("<pre");
  });

  it("普通 fence 仍然保留 <pre>（不然空白会被折叠）", () => {
    const html = render("```ts\nconst a = 1;\n```");
    expect(html).toContain("<pre");
    expect(html).toContain("const a = 1;");
  });

  it("无语言 fence 也保留 <pre>", () => {
    const html = render("```\n┌────┐\n└────┘\n```");
    expect(html).toContain("<pre");
  });
});
