// Identifier underscores in the card's "your last message" preview
// (shared-ts/markdownIdentifiers.ts): `__pycache__` must not render as bold
// `pycache`, while real emphasis still renders.
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import { remarkKeepDunder, stripEmphasisMarkers } from "../../shared-ts/markdownIdentifiers";

const html = (md: string) =>
  renderToStaticMarkup(<ReactMarkdown remarkPlugins={[remarkKeepDunder]}>{md}</ReactMarkdown>);

describe("remarkKeepDunder", () => {
  it("renders a bare dunder name literally", () => {
    expect(html("skip __pycache__ and __init__.py")).toBe("<p>skip __pycache__ and __init__.py</p>");
  });

  it("leaves intraword underscores and real emphasis alone", () => {
    expect(html("test_hello.py, fleet__spawn, **bold**, __two words__, _it_")).toBe(
      "<p>test_hello.py, fleet__spawn, <strong>bold</strong>, <strong>two words</strong>, <em>it</em></p>",
    );
  });

  it("does not touch code", () => {
    expect(html("`__pycache__`")).toBe("<p><code>__pycache__</code></p>");
  });
});

describe("stripEmphasisMarkers", () => {
  it("keeps an unmatched leading underscore", () => {
    expect(stripEmphasisMarkers("call _private_var now")).toBe("call _private_var now");
  });
});
