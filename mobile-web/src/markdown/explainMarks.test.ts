// `[?text]` marks (shared-ts/explainMarks.ts) through the phone's real
// ReactMarkdown chain: the plugin is in `mdRemarkPlugins`, and the span it
// emits has to come out of `mdRehypePlugins`' sanitize pass with its class and
// quote attribute — the desktop pins the same thing for its chain in
// claw-fleet-desktop/app/markdown/explainMarks.test.ts, which also holds the
// mdast-level rule tests (the parser is the same package on both sides).
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import ReactMarkdown from "react-markdown";

import { mdRemarkPlugins, mdRehypePlugins } from "./plugins";

function render(md: string): string {
  return renderToStaticMarkup(
    createElement(ReactMarkdown, {
      remarkPlugins: mdRemarkPlugins,
      rehypePlugins: mdRehypePlugins,
      children: md,
    }),
  );
}

/** Probe A from design/explain-annotations.md §6, five marks. */
const PROBE_A =
  "全部订正完了。四篇 wiki 都已更新，先说最要紧的：**你那个 concern 不只是对的，它推翻了我八条结论里的四条，而且四条全是同一类。**\n\n新写了一篇 `jev/answer-slot-probe-bug` 专门记这个 bug，另外三篇都挂了订正头、换了全部数字。\n\n翻掉的四条，有两条是我今天早上还没查出来的：\n\n**「binary 分解是负面结果，模型不会说 No」——整个诊断是反的。** 旧数据说 4B 只有 80.0%，60 条里 60 条四个 criterion 同时判 Yes，我把它归因为 [?acquiescence bias]。修正后是 **90.0%，multi-positive 只剩 15/60，而且无信号输入上的 Yes−No margin 全是负的（−1.8 到 −3.2）——模型偏向说 No**。那个\"全都说 Yes\"的现象现在只在 cc 校准之后出现，[?是减掉一个负偏置造出来的]，不是模型的毛病。\n\n**「选项顺序轮换是负面结果」——也是反的。** 旧数据是 2B raw 从 86.7% 崩到 71.7%，我还据此说\"默认顺序那 86.7% 里有运气成分\"。修正后轮换在两个尺寸上都帮忙：2B raw 60.0%→76.7%、bc 78.3%→85.0%；4B bc 91.7%→93.3%，其中 4B-p4-cc 拿到全场最高 AUROC **0.960**。延迟 258ms，仍低于 Jev 的 422ms。它当初被判死刑，只是因为被污染的基线虚高。\n\n加上早上那两条（2B 校准\"有害\"、Noul 那个 44 个点的校准神话），一共四条。\n\n**没翻的：** 头条打平的结论更干净了——4B+BC 是 91.7% / AUROC 0.945 / AURC 0.009，和 jev-latest 三个数一模一样，延迟 65ms 对 422ms。三分类钓鱼仍是 Jev 赢，但幅度小了不少：均衡集上从 4.9 点缩到 3.2 点，2024-25 真实钓鱼从我说的\"22–45 点\"变成 7.4–19.6 点。2026 分布外邮件那组 Qwen 反而小胜一个措辞（97.31% 对 97.16%）。\n\n订正过程里冒出来一条新的、我觉得比原来那些更有价值的结论：**Jev 真正稳的地方不是准确率，是不用挑校准方法。** Qwen 在四个数据集上的最优读出换了三次（BC / cc / dc），而 [?单类集合上挑错的代价是 40 个百分点]——而且你没有标签可以事先挑。Jev 十二轮都是同一套输出直接用。\n\n最后是这次最该记住的一条，我写进了 bug 那篇和记忆里：**这个 bug 让 accuracy 最多掉 80 个点，[?AUROC 只动了 0.004]。** 塌掉的四条结论无一例外都是拿 accuracy 讲的故事，所有基于 AUROC 的判断一条没错。我们自己那句\"别用 accuracy 判断未校准分类器的能力\"，是唯一救了这条实验链的东西。检查清单也写进去了：打印 prompt 的 repr、[?算被打分 token 占多少概率质量]（一行代码，应该常驻日志）、让它自由续写看它到底想说什么——这三件事都不能靠推理。\n\n上下文到 255K 了，这段收尾干净，不再往下开新活。";

const PROBE_A_MARKS = [
  "acquiescence bias",
  "是减掉一个负偏置造出来的",
  "单类集合上挑错的代价是 40 个百分点",
  "AUROC 只动了 0.004",
  "算被打分 token 占多少概率质量",
];

describe("explain marks on the mobile chain", () => {
  it("emits the span with class and quote through sanitize", () => {
    const html = render("我把它归因为 [?acquiescence bias]。");
    expect(html).toContain('<span class="explain-mark" data-explain-quote="acquiescence bias">acquiescence bias</span>');
  });

  it("keeps a full-width （ after the mark as text, not a link", () => {
    const html = render("repr、[?算被打分 token 占多少概率质量]（一行代码）");
    expect(html).not.toContain("<a ");
    expect(html).toContain('data-explain-quote="算被打分 token 占多少概率质量"');
    expect(html).toContain("（一行代码）");
  });

  it("leaves inline code and an unterminated [? alone", () => {
    const html = render("`[?code]` 和 [?没有闭合");
    expect(html).toContain("<code>[?code]</code>");
    expect(html).toContain("[?没有闭合");
    expect(html).not.toContain("explain-mark");
  });

  // The run-level scan, on the phone's chain: a mark whose phrase contains
  // inline code, bold or a soft line break used to come out as literal
  // brackets on both sides of the formatted bit.
  it("keeps a mark that wraps inline code", () => {
    const html = render("这不是理论风险——[?`step-code-retire` 那条计划的 P1 就在敲救这个]：线上没有。");
    expect(html).toContain('data-explain-quote="step-code-retire 那条计划的 P1 就在敲救这个"');
    expect(html).toContain("<code>step-code-retire</code>");
    expect(html).not.toContain("[?");
  });

  it("keeps a mark that wraps bold or a soft line break", () => {
    const bold = render("风险在 [?**四条结论**全是同一类] 这里。");
    expect(bold).toContain('data-explain-quote="四条结论全是同一类"');
    expect(bold).toContain("<strong>四条结论</strong>");
    const broken = render("风险在 [?四条结论\n全是同一类] 这里。");
    expect(broken).toContain('class="explain-mark"');
    expect(broken).not.toContain("[?");
  });

  it("leaves a range containing a link literal", () => {
    const html = render("见 [?这里 [文档](https://a.b) 说了] 。");
    expect(html).not.toContain("explain-mark");
    expect(html).toContain("[?这里");
  });

  it("keeps a bracketed index inside the mark", () => {
    const html = render("见 [?数组 a[0] 的值] 那里");
    expect(html).toContain('data-explain-quote="数组 a[0] 的值"');
  });

  it("renders all five probe-A marks", () => {
    const html = render(PROBE_A);
    expect(html.match(/class="explain-mark"/g)).toHaveLength(5);
    for (const q of PROBE_A_MARKS) expect(html).toContain(`data-explain-quote="${q}"`);
  });

  it("drops a handler on a raw span that fakes the class", () => {
    const html = render('<span class="explain-mark" onclick="alert(1)" data-explain-quote="x">x</span>');
    expect(html).toContain('class="explain-mark"');
    expect(html).not.toContain("onclick");
  });
});
