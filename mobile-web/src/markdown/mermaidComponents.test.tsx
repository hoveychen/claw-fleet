import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import { mermaidMarkdownComponents } from "./mermaidComponents";
import { MD_BLOCK, MD_INLINE } from "../views/SessionDetailTabs";

// MermaidBlock reads html[data-theme] during render to pick the theme. renderToStaticMarkup
// doesn't run effects, so this single read is its entire DOM need — stub it with one line,
// no need to add jsdom to mobile-web (only desktop has it).
vi.stubGlobal("document", { documentElement: { getAttribute: () => "light" } });

const MERMAID = "```mermaid\nflowchart TB\n  A --> B\n```";
const TS = "```ts\nconst a = 1;\n```";

function render(md: string, components: Components): string {
  return renderToStaticMarkup(
    <ReactMarkdown components={components}>{md}</ReactMarkdown>,
  );
}

/** Diagram rendering is async (mermaid is lazy-loaded), so the server's first frame has only
 *  an empty container div. The test criterion is "fence not rendered as a code block", not
 *  "whether <svg> exists". */
function rendersDiagram(html: string): boolean {
  return !html.includes("language-mermaid") && !html.includes("flowchart TB");
}

describe("mermaidMarkdownComponents", () => {
  it("mermaid fence becomes diagram container, no longer a code block", () => {
    expect(rendersDiagram(render(MERMAID, mermaidMarkdownComponents))).toBe(true);
  });

  it("Regular fences remain code blocks with <pre>", () => {
    const html = render(TS, mermaidMarkdownComponents);
    expect(html).toContain("<pre");
    expect(html).toContain("const a = 1;");
  });
});

// Decision/plan tabs once failed to handle mermaid (component map only covered 'a'), yet comments
// claimed "same as wiki/message view". These two tests lock that down.
describe("SessionDetailTabs decision content component map", () => {
  it("MD_BLOCK recognizes mermaid", () => {
    expect(rendersDiagram(render(MERMAID, MD_BLOCK))).toBe(true);
  });

  it("MD_INLINE also recognizes it (diagrams can appear in option labels)", () => {
    expect(rendersDiagram(render(MERMAID, MD_INLINE))).toBe(true);
  });

  // We used to assert links must be inert <span>: the bug where external links wouldn't open on mobile
  // lived under this assertion. Now external links go to the system browser (handled by launchIntent /
  // onLoadIntercept in the shell); only unrecognized schemes remain non-clickable.
  it("External links are real <a>, unknown schemes remain non-clickable", () => {
    expect(render("[x](https://example.com)", MD_BLOCK)).toContain("<a ");
    expect(render("[x](./a.md)", MD_BLOCK)).not.toContain("href=");
  });
});
