import { describe, it, expect } from "vitest";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import rehypeStringify from "rehype-stringify";
import { safeRemarkPlugins, safeRehypePlugins } from "./plugins";

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

  it("keeps the language class a mermaid block is recognised by", () => {
    expect(render("```mermaid\ngraph TD;\nA-->B;\n```")).toContain("language-mermaid");
  });
});
