import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import ReactMarkdown from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins, normalizeSvgBlankLines } from "./plugins";

/**
 * This chain previously had tests only on desktop, so it drifted silently on two counts —
 * `singleTilde` wasn't turned off, and `remarkCjkAutolinkFix` is missing entirely on mobile.
 * Both were hit by the desktop first, fixed and tested there, while mobile stayed broken
 * because it had no corresponding tests.
 *
 * So this tests not "can markdown render" but those two specific regressions. The desktop's
 * counterpart is claw-fleet-desktop/app/markdown/plugins.test.ts.
 */
function render(md: string): string {
  return renderToStaticMarkup(
    createElement(ReactMarkdown, {
      remarkPlugins: mdRemarkPlugins,
      rehypePlugins: mdRehypePlugins,
      children: md,
    }),
  );
}

describe("tilde doesn't consume home paths", () => {
  // When two `~/` paths appear in one message, remark-gfm's singleTilde default
  // swallows everything between them into one <del>.
  it("content between two ~/ paths isn't rendered as strikethrough", () => {
    const html = render(
      "watcher 监听着 ~/.claude/skills、开关=true，但 Codex 目录写错(~/.agents→~/.codex/skills)",
    );
    expect(html).not.toContain("<del>");
    expect(html).toContain("~/.claude/skills");
    expect(html).toContain("~/.agents");
    expect(html).toContain("~/.codex/skills");
  });

  it("GFM standard ~~strikethrough~~ still works", () => {
    expect(render("~~x~~")).toContain("<del>x</del>");
  });
});

describe("CJK not included in autolinks", () => {
  it("CJK punctuation terminates URL, not in href", () => {
    const html = render("见 https://example.com，然后回来");
    expect(html).toContain('href="https://example.com"');
    expect(html).not.toContain("example.com，");
    expect(html).toContain("，然后回来");
  });

  it("pure ASCII context autolinks unaffected", () => {
    expect(render("see https://example.com/a/b thanks")).toContain(
      'href="https://example.com/a/b"',
    );
  });
});

describe("other paths unchanged", () => {
  it("bold works when CJK is adjacent to punctuation", () => {
    expect(render("一个是**“引号开头”的加粗**。后面")).toContain(
      "<strong>“引号开头”的加粗</strong>",
    );
  });

  it("table alignment and task list checkboxes present", () => {
    // react-markdown encodes mdast alignment as inline style, not `align` attribute
    // (desktop test goes through rehype-stringify directly, so it asserts `align="center"`).
    expect(render("| a |\n|:-:|\n| 1 |")).toContain("text-align:center");
    expect(render("- [x] done")).toContain('type="checkbox"');
  });

  it("inline SVG survives sanitize", () => {
    const html = render('<svg viewBox="0 0 10 10"><rect x="1" y="1" width="4" height="4"/></svg>');
    expect(html).toContain("<svg");
    expect(html).toContain("<rect");
  });
});

// Mirrors the desktop's "inline SVG url(#id) references" cases: sanitize prefixes
// `id="arr"` to `user-content-arr` but left `marker-end="url(#arr)"` alone, so
// every arrowhead and gradient resolved to nothing.
describe("inline SVG url(#id) references follow the id prefix", () => {
  const svg = [
    '<svg viewBox="0 0 100 50" xmlns="http://www.w3.org/2000/svg">',
    '<defs><marker id="arr" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto"><path d="M0,0 L6,3 L0,6 z"/></marker>',
    '<linearGradient id="g"><stop offset="0" stop-color="#fff"/></linearGradient></defs>',
    '<rect width="10" height="10" fill="url(#g)"/>',
    '<line x1="0" y1="0" x2="50" y2="0" stroke="#000" marker-end="url(#arr)"/>',
    "</svg>",
  ].join("\n");

  it("points marker and paint references at the prefixed ids", () => {
    const html = render(svg);
    expect(html).toContain('id="user-content-arr"');
    expect(html).toContain('marker-end="url(#user-content-arr)"');
    expect(html).toContain('fill="url(#user-content-g)"');
    expect(html).not.toMatch(/url\(\s*['"]?#(?!user-content-)/);
  });

  it("does not double-prefix, and leaves non-fragment urls alone", () => {
    const html = render(svg.replace('url(#g)"', 'url(#user-content-g)"'));
    expect(html).not.toContain("user-content-user-content-");
    expect(render('<svg viewBox="0 0 10 10"><rect width="10" height="10" fill="url(https://x.test/a)"/></svg>'))
      .not.toContain("user-content-https");
  });
});

describe("<style> content is dropped, not leaked as text", () => {
  it("strips the CSS along with the tag", () => {
    const html = render("段落\n\n<style>.t{font:12px sans-serif;fill:#333}</style>\n\n结束");
    expect(html).not.toContain("<style");
    expect(html).not.toContain("font:12px");
    expect(html).toContain("结束");
  });
});

// Mirrors the desktop's "inline SVG blank lines" / "content after the open tag"
// cases. This chain had no normalization at all, so a blank line inside an <svg>
// truncated the drawing on every mobile surface.
describe("inline SVG blank lines and same-line open tags", () => {
  const svg = [
    '<svg viewBox="0 0 200 100" xmlns="http://www.w3.org/2000/svg">',
    '  <rect width="200" height="100" fill="#fafafa"/>',
    "",
    '  <text x="10" y="20">label</text>',
    '  <circle cx="50" cy="50" r="20" fill="#f59e0b"/>',
    "</svg>",
  ].join("\n");
  const inner = (html: string) => html.slice(html.indexOf("<svg"), html.indexOf("</svg>"));

  it("truncates at the blank line without normalization (documents the bug)", () => {
    expect(inner(render(svg))).not.toContain("#f59e0b");
  });

  it("keeps the whole drawing once normalized", () => {
    const html = render(normalizeSvgBlankLines(svg));
    expect(inner(html)).toContain("#f59e0b");
    expect(html).not.toContain("<p>");
  });

  it("moves content after a same-line open tag to its own line", () => {
    const md = '<svg viewBox="0 0 10 10" aria-label="a > b"><rect width="1" height="1"/></svg>';
    expect(normalizeSvgBlankLines(md)).toBe(
      '<svg viewBox="0 0 10 10" aria-label="a > b">\n<rect width="1" height="1"/></svg>',
    );
  });

  it("leaves fenced code and prose-only mentions untouched", () => {
    const fenced = "```html\n" + svg + "\n```";
    expect(normalizeSvgBlankLines(fenced)).toBe(fenced);
    const prose = "先说 `<svg>` 标签。\n\n第一段。\n\n第二段。";
    expect(normalizeSvgBlankLines(prose)).toBe(prose);
  });
});
