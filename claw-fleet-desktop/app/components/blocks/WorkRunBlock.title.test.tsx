import { describe, it, expect } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import { safeRemarkPlugins, safeRehypePlugins } from "../../markdown/safeLinks";
import { titleMarkdownComponents } from "./WorkRunBlock";

/** Render a band title exactly as WorkRunBlock does. */
function renderTitle(title: string): string {
  return renderToStaticMarkup(
    <ReactMarkdown
      remarkPlugins={safeRemarkPlugins}
      rehypePlugins={safeRehypePlugins}
      components={titleMarkdownComponents}
    >
      {title}
    </ReactMarkdown>,
  );
}

describe("work-run band title markdown", () => {
  it("renders **bold** as <strong>, not literal asterisks", () => {
    const html = renderTitle("**Planning store test additions**");
    expect(html).toContain("<strong>Planning store test additions</strong>");
    expect(html).not.toContain("**");
  });

  it("stays inline — no wrapping <p> block", () => {
    const html = renderTitle("**Planning store test additions**");
    expect(html).not.toContain("<p>");
  });

  it("renders inline `code` spans", () => {
    const html = renderTitle("Wiring the `serve` endpoint");
    expect(html).toContain("<code");
    expect(html).not.toContain("`");
  });

  it("passes plain prose through untouched", () => {
    const html = renderTitle("Reviewing the middleware code");
    expect(html).toContain("Reviewing the middleware code");
  });
});
