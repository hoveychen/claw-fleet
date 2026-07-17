import { describe, it, expect } from "vitest";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import rehypeStringify from "rehype-stringify";
import { safeRemarkPlugins, safeRehypePlugins, normalizeSvgBlankLines } from "./plugins";

/** Render through the exact chain the app's ReactMarkdown instances use. */
function render(md: string): string {
  return String(
    unified()
      .use(remarkParse)
      .use(safeRemarkPlugins)
      // `allowDangerousHtml` is what react-markdown does internally; rehype-raw
      // needs the raw nodes to still be in the tree when it runs.
      .use(remarkRehype, { allowDangerousHtml: true })
      .use(safeRehypePlugins)
      .use(rehypeStringify, { allowDangerousHtml: true })
      .processSync(md),
  );
}

describe("CJK emphasis", () => {
  // The bug that started this: CommonMark won't open emphasis when `**` sits
  // between a CJK character and punctuation, so the asterisks leaked as text.
  it("bolds when ** is glued to a CJK char on the left and a quote on the right", () => {
    const html = render("一个是**“一年一两次、口径每年变”这个特征本身**。所有 SaaS");
    expect(html).toContain("<strong>“一年一两次、口径每年变”这个特征本身</strong>");
    expect(html).not.toContain("**");
  });

  it("still bolds the plain CJK case", () => {
    expect(render("前面**加粗内容**后面")).toContain("<strong>加粗内容</strong>");
  });

  it("bolds a run that both opens and closes against CJK punctuation", () => {
    const html = render("他说：**“不行”**，然后走了");
    expect(html).toContain("<strong>“不行”</strong>");
  });
});

describe("math", () => {
  it("typesets inline math with KaTeX", () => {
    const html = render("质能方程 $E=mc^2$ 很有名");
    expect(html).toContain("katex");
    expect(html).not.toContain("$E=mc^2$");
  });

  it("typesets display math", () => {
    expect(render("$$\n\\frac{a}{b}\n$$")).toContain("katex-display");
  });
});

describe("raw HTML", () => {
  it("renders benign inline tags", () => {
    expect(render("H<sub>2</sub>O")).toContain("<sub>2</sub>");
  });

  it("strips script tags", () => {
    const html = render("hi <script>alert(1)</script> there");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("alert(1)");
  });

  it("strips inline event handlers and javascript: hrefs", () => {
    const html = render('<a href="javascript:alert(1)" onclick="alert(2)">x</a>');
    expect(html).not.toContain("onclick");
    expect(html).not.toContain("javascript:");
  });

  it("keeps an img but drops its onerror handler", () => {
    const html = render('<img src="https://example.com/a.png" onerror="alert(1)">');
    expect(html).toContain("<img");
    expect(html).not.toContain("onerror");
  });
});

describe("inline SVG blank lines", () => {
  // A bare <svg> opens a CommonMark type-7 HTML block, which ends at the first
  // blank line. A model that groups an SVG's sections with blank lines truncates
  // the drawing — everything after the blank line escapes the <svg>.
  // The blank line closes the type-7 HTML block; the following <text> line is a
  // complete open tag *not* followed by only whitespace, so remark parses it as
  // a paragraph — the resulting <p> severs rehype-raw's re-stitching and the
  // <svg> auto-closes early. This mirrors the real "写入/擦除" diagram.
  const svg = [
    '<svg viewBox="0 0 200 100" xmlns="http://www.w3.org/2000/svg">',
    '  <rect x="0" y="0" width="200" height="100" fill="#fafafa"/>',
    '',
    '  <text x="10" y="20" fill="#16a34a">label</text>',
    '  <circle cx="50" cy="50" r="20" fill="#f59e0b"/>',
    "</svg>",
  ].join("\n");

  // Extract the substring between the first <svg and its </svg>.
  const svgInner = (html: string) => {
    const start = html.indexOf("<svg");
    const end = html.indexOf("</svg>");
    return start >= 0 && end >= 0 ? html.slice(start, end) : "";
  };

  it("truncates at the blank line without the fix (documents the bug)", () => {
    const inner = svgInner(render(svg));
    // The late <circle> escapes the <svg> and lands in a paragraph.
    expect(inner).not.toContain("#f59e0b");
    expect(render(svg)).toContain("<p>");
  });

  it("keeps the whole drawing when blank lines are normalized", () => {
    const inner = svgInner(render(normalizeSvgBlankLines(svg)));
    expect(inner).toContain("#fafafa");
    expect(inner).toContain("#f59e0b");
    // Nothing escaped into a loose paragraph.
    expect(render(normalizeSvgBlankLines(svg))).not.toContain("<p>");
  });

  it("leaves an SVG shown inside a code fence untouched", () => {
    const fenced = "```html\n" + svg + "\n```";
    expect(normalizeSvgBlankLines(fenced)).toBe(fenced);
  });

  it("is a no-op for text with no svg", () => {
    const md = "line one\n\nline two\n";
    expect(normalizeSvgBlankLines(md)).toBe(md);
  });
});

describe("GFM survives the sanitize pass", () => {
  it("keeps table alignment", () => {
    const html = render("| a |\n|:-:|\n| 1 |");
    expect(html).toContain("<table>");
    expect(html).toContain('align="center"');
  });

  it("keeps task-list checkboxes", () => {
    const html = render("- [x] done");
    expect(html).toContain('type="checkbox"');
  });

  it("keeps strikethrough and autolinks", () => {
    expect(render("~~x~~")).toContain("<del>x</del>");
    expect(render("见 https://example.com 谢谢")).toContain('href="https://example.com"');
  });

  it("does not treat bare home-directory paths as strikethrough", () => {
    const html = render(
      "watcher 监听着 ~/.claude/skills、开关=true，但 Codex 目录写错(~/.agents→~/.codex/skills)",
    );
    expect(html).not.toContain("<del>");
    expect(html).toContain("~/.claude/skills");
    expect(html).toContain("~/.agents");
    expect(html).toContain("~/.codex/skills");
  });

  it("keeps the language class a mermaid block is recognised by", () => {
    expect(render("```mermaid\ngraph TD;\nA-->B;\n```")).toContain("language-mermaid");
  });
});

// The chat brief promises "内联 HTML/SVG…它会真实渲染出来", and a model asked to
// diagram a circuit answers with inline <svg>. GitHub's default sanitize schema
// allows no SVG tags at all, so without widening every <svg>/<rect>/<line>/…
// is stripped and only the <text> content survives — flowing into a run-on
// paragraph. That is the exact "diagram collapsed to one line" bug.
describe("inline SVG survives the sanitize pass", () => {
  const svg = [
    '<svg viewBox="0 0 200 100" xmlns="http://www.w3.org/2000/svg">',
    '  <rect x="10" y="10" width="60" height="40" fill="#dbeafe" stroke="#1a1a1a"/>',
    '  <line x1="10" y1="70" x2="190" y2="70" stroke="#2563eb" stroke-width="2"/>',
    '  <circle cx="100" cy="30" r="5" fill="#16a34a"/>',
    '  <path d="M120 30 q40 10 55 30" stroke="#dc2626" fill="none"/>',
    '  <polygon points="150,60 156,53 146,51" fill="#dc2626"/>',
    '  <text x="20" y="35" font-size="13" fill="#1a1a1a">开关管</text>',
    "</svg>",
  ].join("\n");

  it("keeps the svg element and its shape children", () => {
    const html = render(svg);
    expect(html).toContain("<svg");
    expect(html).toContain("<rect");
    expect(html).toContain("<line");
    expect(html).toContain("<circle");
    expect(html).toContain("<path");
    expect(html).toContain("<polygon");
    expect(html).toContain("<text");
  });

  it("keeps the geometry and presentation attributes a diagram needs", () => {
    const html = render(svg);
    expect(html).toContain('viewBox="0 0 200 100"');
    expect(html).toContain('fill="#dbeafe"');
    expect(html).toContain('stroke="#1a1a1a"');
    expect(html).toContain('d="M120 30 q40 10 55 30"');
    expect(html).toContain('points="150,60 156,53 146,51"');
    expect(html).toContain(">开关管</text>");
  });

  it("still strips script and event handlers inside svg", () => {
    const html = render(
      '<svg viewBox="0 0 10 10"><script>alert(1)</script>' +
        '<rect width="10" height="10" onload="alert(2)"/></svg>',
    );
    expect(html).toContain("<svg");
    expect(html).toContain("<rect");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("alert(1)");
    expect(html).not.toContain("onload");
  });

  it("strips foreignObject, the SVG→HTML script escape hatch", () => {
    const html = render(
      '<svg viewBox="0 0 10 10"><foreignObject>' +
        '<img src=x onerror="alert(1)"></foreignObject></svg>',
    );
    expect(html).not.toContain("<foreignObject");
    expect(html).not.toContain("onerror");
  });
});
