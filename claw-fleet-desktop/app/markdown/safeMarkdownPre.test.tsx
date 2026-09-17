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

describe("safeMarkdownComponents overrides pre rendering", () => {
  it("mermaid fence is no longer wrapped in outer <pre>", () => {
    // <pre> wraps the diagram in a monospace font box — mermaid measures tag
    // widths in sans-serif, so when inherited into monospace it can't fit and
    // gets clipped by the node box.
    const html = render("```mermaid\nflowchart TB\n  A --> B\n```");
    expect(html).not.toContain("<pre");
  });

  it("regular fence keeps <pre> (otherwise whitespace gets collapsed)", () => {
    const html = render("```ts\nconst a = 1;\n```");
    expect(html).toContain("<pre");
    expect(html).toContain("const a = 1;");
  });

  it("fence without language also keeps <pre>", () => {
    const html = render("```\n┌────┐\n└────┘\n```");
    expect(html).toContain("<pre");
  });
});
