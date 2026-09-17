import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import ReactMarkdown from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "./plugins";

/**
 * 这条链此前只在桌面侧有测试，于是它悄悄漂移了两项 —— `singleTilde` 没关、
 * `remarkCjkAutolinkFix` 整个文件都不在手机端。两处都是桌面先撞到、修好、
 * 写了测试，而手机端因为没有对应的测试，一直保持在出 bug 的状态。
 *
 * 所以这里测的不是"markdown 能不能渲染"，而是那两处具体的回归。桌面侧的对照
 * 文件是 claw-fleet-desktop/app/markdown/plugins.test.ts。
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

describe("波浪线不吃掉 home 路径", () => {
  // 一条消息里出现两个 `~/` 开头的路径时，remark-gfm 的 singleTilde 默认值
  // 会把两者之间的全部内容吞进一个 <del>。
  it("两个 ~/ 路径之间的内容不被渲染成删除线", () => {
    const html = render(
      "watcher 监听着 ~/.claude/skills、开关=true，但 Codex 目录写错(~/.agents→~/.codex/skills)",
    );
    expect(html).not.toContain("<del>");
    expect(html).toContain("~/.claude/skills");
    expect(html).toContain("~/.agents");
    expect(html).toContain("~/.codex/skills");
  });

  it("GFM 标准的 ~~删除线~~ 仍然有效", () => {
    expect(render("~~x~~")).toContain("<del>x</del>");
  });
});

describe("CJK 不被卷进自动链接", () => {
  it("中文标点终止 URL，不进 href", () => {
    const html = render("见 https://example.com，然后回来");
    expect(html).toContain('href="https://example.com"');
    expect(html).not.toContain("example.com，");
    expect(html).toContain("，然后回来");
  });

  it("纯 ASCII 上下文的自动链接不受影响", () => {
    expect(render("see https://example.com/a/b thanks")).toContain(
      'href="https://example.com/a/b"',
    );
  });
});

describe("其余链路保持原样", () => {
  it("CJK 紧贴标点时仍能加粗", () => {
    expect(render("一个是**“引号开头”的加粗**。后面")).toContain(
      "<strong>“引号开头”的加粗</strong>",
    );
  });

  it("表格对齐与任务列表复选框都在", () => {
    // react-markdown 把 mdast 的对齐信息编成行内 style，而不是 `align` 属性
    // （桌面那份测试直接走 rehype-stringify，所以断言的是 `align="center"`）。
    expect(render("| a |\n|:-:|\n| 1 |")).toContain("text-align:center");
    expect(render("- [x] done")).toContain('type="checkbox"');
  });

  it("内联 SVG 扛过 sanitize", () => {
    const html = render('<svg viewBox="0 0 10 10"><rect x="1" y="1" width="4" height="4"/></svg>');
    expect(html).toContain("<svg");
    expect(html).toContain("<rect");
  });
});
