// The tappable half of `[?text]` marks on the phone: `mdComponents.span` maps
// the plugin's span to `ExplainMarkSpan`, which is a button inside an
// `ExplainMarksProvider` and plain text outside one. The mobile test runner has
// no DOM, so the click path (row → anchor, user rows inert) is pinned on the
// desktop, which shares the same rules; here the static markup is.
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import ReactMarkdown from "react-markdown";

import { mdComponents } from "./components";
import { ExplainMarksProvider } from "./explainMarks";
import { mdRemarkPlugins, mdRehypePlugins } from "./plugins";

const MD = "旧数据我把它归因为 [?acquiescence bias]。";

function render(md: string, withProvider: boolean): string {
  const body = createElement(ReactMarkdown, {
    remarkPlugins: mdRemarkPlugins,
    rehypePlugins: mdRehypePlugins,
    components: mdComponents,
    children: md,
  });
  return renderToStaticMarkup(
    withProvider ? createElement(ExplainMarksProvider, { value: { onMark: () => {} }, children: body }) : body,
  );
}

describe("ExplainMarkSpan via mdComponents", () => {
  it("is a button carrying the quote inside a provider", () => {
    const html = render(MD, true);
    expect(html).toContain('role="button"');
    expect(html).toContain('data-explain-quote="acquiescence bias"');
    expect(html).toContain(">acquiescence bias</span>");
    expect(html).not.toContain("[?");
  });

  it("is plain text outside a provider", () => {
    const html = render(MD, false);
    expect(html).not.toContain('role="button"');
    expect(html).not.toContain("data-explain-quote");
    expect(html).toContain("acquiescence bias");
    expect(html).not.toContain("[?");
  });

  it("leaves KaTeX's spans alone", () => {
    const html = render("质能方程 $E=mc^2$", true);
    expect(html).toContain("katex");
    expect(html).not.toContain('role="button"');
  });
});
