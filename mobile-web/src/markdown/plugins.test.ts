import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import ReactMarkdown from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "./plugins";

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
