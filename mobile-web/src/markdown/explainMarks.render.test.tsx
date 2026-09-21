// The tappable half of `[?text]` marks on the phone: `mdComponents.span` maps
// the plugin's span to `ExplainMarkSpan`. Whether a mark is tappable is decided
// from the DOM after mount (it has to sit in an assistant row), so the static
// markup here shows the pre-mount state — the quote carried, the brackets gone,
// KaTeX untouched. The click path (selection covers the mark, the bar shows,
// user rows inert) is pinned on the desktop, which shares the rule and has a DOM
// in its test runner.
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import ReactMarkdown from "react-markdown";

import { mdComponents } from "./components";
import { mdRemarkPlugins, mdRehypePlugins } from "./plugins";

function render(md: string): string {
  return renderToStaticMarkup(
    createElement(ReactMarkdown, {
      remarkPlugins: mdRemarkPlugins,
      rehypePlugins: mdRehypePlugins,
      components: mdComponents,
      children: md,
    }),
  );
}

describe("ExplainMarkSpan via mdComponents", () => {
  it("carries the quote and drops the brackets", () => {
    const html = render("旧数据我把它归因为 [?acquiescence bias]。");
    expect(html).toContain('data-explain-quote="acquiescence bias"');
    expect(html).toContain(">acquiescence bias</span>");
    expect(html).not.toContain("[?");
    // Not a button before the DOM says it sits in agent prose.
    expect(html).not.toContain('role="button"');
  });

  it("leaves KaTeX's spans alone", () => {
    const html = render("质能方程 $E=mc^2$");
    expect(html).toContain("katex");
    expect(html).not.toContain("data-explain-quote");
  });
});
