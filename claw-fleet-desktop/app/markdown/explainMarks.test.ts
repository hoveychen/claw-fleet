// `[?text]` marks in agent prose (shared-ts/explainMarks.ts): the mdast rules
// the plugin enforces, and the proof that the span it emits survives this
// package's real remark → rehype-raw → rehype-sanitize chain with its class and
// quote attribute intact. The mobile chain has its own copy of the latter
// (mobile-web/src/markdown/explainMarks.test.ts).
import { describe, expect, it } from "vitest";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkGfm from "remark-gfm";
import remarkRehype from "remark-rehype";
import rehypeStringify from "rehype-stringify";

import {
  EXPLAIN_MARK_CLASS,
  remarkExplainMarks,
  splitExplainMarks,
  stripExplainMarks,
} from "../../../shared-ts/explainMarks";
import { safeRehypePlugins, safeRemarkPlugins } from "./plugins";

/** Probe A from design/explain-annotations.md §6 — a real Opus 5 reply
 *  rewritten with exactly five marks, all inside the guidance's guard rails. */
const PROBE_A =
  "全部订正完了。四篇 wiki 都已更新，先说最要紧的：**你那个 concern 不只是对的，它推翻了我八条结论里的四条，而且四条全是同一类。**\n\n新写了一篇 `jev/answer-slot-probe-bug` 专门记这个 bug，另外三篇都挂了订正头、换了全部数字。\n\n翻掉的四条，有两条是我今天早上还没查出来的：\n\n**「binary 分解是负面结果，模型不会说 No」——整个诊断是反的。** 旧数据说 4B 只有 80.0%，60 条里 60 条四个 criterion 同时判 Yes，我把它归因为 [?acquiescence bias]。修正后是 **90.0%，multi-positive 只剩 15/60，而且无信号输入上的 Yes−No margin 全是负的（−1.8 到 −3.2）——模型偏向说 No**。那个\"全都说 Yes\"的现象现在只在 cc 校准之后出现，[?是减掉一个负偏置造出来的]，不是模型的毛病。\n\n**「选项顺序轮换是负面结果」——也是反的。** 旧数据是 2B raw 从 86.7% 崩到 71.7%，我还据此说\"默认顺序那 86.7% 里有运气成分\"。修正后轮换在两个尺寸上都帮忙：2B raw 60.0%→76.7%、bc 78.3%→85.0%；4B bc 91.7%→93.3%，其中 4B-p4-cc 拿到全场最高 AUROC **0.960**。延迟 258ms，仍低于 Jev 的 422ms。它当初被判死刑，只是因为被污染的基线虚高。\n\n加上早上那两条（2B 校准\"有害\"、Noul 那个 44 个点的校准神话），一共四条。\n\n**没翻的：** 头条打平的结论更干净了——4B+BC 是 91.7% / AUROC 0.945 / AURC 0.009，和 jev-latest 三个数一模一样，延迟 65ms 对 422ms。三分类钓鱼仍是 Jev 赢，但幅度小了不少：均衡集上从 4.9 点缩到 3.2 点，2024-25 真实钓鱼从我说的\"22–45 点\"变成 7.4–19.6 点。2026 分布外邮件那组 Qwen 反而小胜一个措辞（97.31% 对 97.16%）。\n\n订正过程里冒出来一条新的、我觉得比原来那些更有价值的结论：**Jev 真正稳的地方不是准确率，是不用挑校准方法。** Qwen 在四个数据集上的最优读出换了三次（BC / cc / dc），而 [?单类集合上挑错的代价是 40 个百分点]——而且你没有标签可以事先挑。Jev 十二轮都是同一套输出直接用。\n\n最后是这次最该记住的一条，我写进了 bug 那篇和记忆里：**这个 bug 让 accuracy 最多掉 80 个点，[?AUROC 只动了 0.004]。** 塌掉的四条结论无一例外都是拿 accuracy 讲的故事，所有基于 AUROC 的判断一条没错。我们自己那句\"别用 accuracy 判断未校准分类器的能力\"，是唯一救了这条实验链的东西。检查清单也写进去了：打印 prompt 的 repr、[?算被打分 token 占多少概率质量]（一行代码，应该常驻日志）、让它自由续写看它到底想说什么——这三件事都不能靠推理。\n\n上下文到 255K 了，这段收尾干净，不再往下开新活。";

const PROBE_A_MARKS = [
  "acquiescence bias",
  "是减掉一个负偏置造出来的",
  "单类集合上挑错的代价是 40 个百分点",
  "AUROC 只动了 0.004",
  "算被打分 token 占多少概率质量",
];

interface Node {
  type: string;
  value?: string;
  children?: Node[];
  data?: { hName?: string; hProperties?: Record<string, unknown> };
}

/** Parse with GFM (the tokenizer the app uses) and run only the mark plugin —
 *  the rules are the plugin's, not the rest of the chain's. */
function parse(md: string, withPlugin = true): Node {
  const proc = unified().use(remarkParse).use(remarkGfm);
  if (withPlugin) proc.use(remarkExplainMarks);
  return proc.runSync(proc.parse(md)) as unknown as Node;
}

function collect(node: Node, type: string, out: Node[] = []): Node[] {
  if (node.type === type) out.push(node);
  for (const c of node.children ?? []) collect(c, type, out);
  return out;
}

function quoteOf(mark: Node): unknown {
  return mark.data?.hProperties?.dataExplainQuote;
}

/** The exact chain every desktop ReactMarkdown runs (`safeRemarkPlugins`
 *  already carries the mark plugin). */
function render(md: string): string {
  return String(
    unified()
      .use(remarkParse)
      .use(safeRemarkPlugins)
      .use(remarkRehype, { allowDangerousHtml: true })
      .use(safeRehypePlugins)
      .use(rehypeStringify, { allowDangerousHtml: true })
      .processSync(md),
  );
}

describe("remarkExplainMarks: mdast rules", () => {
  it("turns a plain mark into an explainMark node carrying the quote", () => {
    const tree = parse("前面 [?acquiescence bias] 后面");
    const marks = collect(tree, "explainMark");
    expect(marks).toHaveLength(1);
    expect(marks[0].data?.hName).toBe("span");
    expect(marks[0].data?.hProperties?.className).toEqual([EXPLAIN_MARK_CLASS]);
    expect(quoteOf(marks[0])).toBe("acquiescence bias");
    expect(marks[0].children).toEqual([{ type: "text", value: "acquiescence bias" }]);
    // The surrounding prose is still there, split around the mark.
    const texts = collect(tree, "text").map((t) => t.value);
    expect(texts).toEqual(["前面 ", "acquiescence bias", " 后面"]);
  });

  it("keeps CJK and digits together inside one mark", () => {
    const marks = collect(parse("而 [?单类集合上挑错的代价是 40 个百分点]——而且"), "explainMark");
    expect(marks).toHaveLength(1);
    expect(quoteOf(marks[0])).toBe("单类集合上挑错的代价是 40 个百分点");
  });

  it("is not fooled by a full-width （ right after the mark", () => {
    // A half-width `(` would make CommonMark read `[…](…)` as a link; the
    // guidance forbids that, and the full-width form it allows must stay a mark.
    const tree = parse("repr、[?算被打分 token 占多少概率质量]（一行代码）、然后");
    expect(collect(tree, "link")).toHaveLength(0);
    const marks = collect(tree, "explainMark");
    expect(marks).toHaveLength(1);
    expect(quoteOf(marks[0])).toBe("算被打分 token 占多少概率质量");
    expect(collect(tree, "text").map((t) => t.value)).toContain("（一行代码）、然后");
  });

  it("leaves an unterminated [? as literal text", () => {
    const tree = parse("这里 [?没有闭合 的一句话");
    expect(collect(tree, "explainMark")).toHaveLength(0);
    expect(collect(tree, "text").map((t) => t.value).join("")).toBe("这里 [?没有闭合 的一句话");
  });

  it("does not touch a mark written inside inline code", () => {
    const tree = parse("代码 `[?not a mark]` 之外 [?a mark]");
    const marks = collect(tree, "explainMark");
    expect(marks).toHaveLength(1);
    expect(quoteOf(marks[0])).toBe("a mark");
    expect(collect(tree, "inlineCode")[0].value).toBe("[?not a mark]");
  });

  it("does not rewrite link text", () => {
    const tree = parse("[看 [?这里]](https://example.com) 与 [?外面]");
    const marks = collect(tree, "explainMark");
    expect(marks).toHaveLength(1);
    expect(quoteOf(marks[0])).toBe("外面");
  });

  it("splits two marks in one text node, in order", () => {
    const tree = parse("先 [?第一处] 然后 [?第二处] 结束");
    const marks = collect(tree, "explainMark");
    expect(marks.map(quoteOf)).toEqual(["第一处", "第二处"]);
    expect(collect(tree, "text").map((t) => t.value)).toEqual([
      "先 ",
      "第一处",
      " 然后 ",
      "第二处",
      " 结束",
    ]);
  });

  it("treats a nested [? literally, closing the outer mark at the first ]", () => {
    const marks = collect(parse("[?外层 [?内层] 尾巴]"), "explainMark");
    expect(marks).toHaveLength(1);
    expect(quoteOf(marks[0])).toBe("外层 [?内层");
  });

  it("returns an identical tree for text without marks", () => {
    const md = "没有标注的 **段落**，带 `code` 和 [链接](https://a.b)。\n\n- 列表\n- 项目";
    const before = parse(md, false);
    const after = parse(md);
    expect(JSON.parse(JSON.stringify(after))).toEqual(JSON.parse(JSON.stringify(before)));
  });

  it("finds exactly the five marks of probe A", () => {
    const marks = collect(parse(PROBE_A), "explainMark");
    expect(marks.map(quoteOf)).toEqual(PROBE_A_MARKS);
  });
});

describe("splitExplainMarks / stripExplainMarks", () => {
  it("ignores an empty [?]", () => {
    expect(splitExplainMarks("a [?] b")).toEqual([{ kind: "text", value: "a [?] b" }]);
  });

  it("strips the brackets and keeps the text for plain-text surfaces", () => {
    expect(stripExplainMarks("我把它归因为 [?acquiescence bias]。")).toBe("我把它归因为 acquiescence bias。");
    expect(stripExplainMarks("no marks")).toBe("no marks");
    expect(stripExplainMarks("[?open")).toBe("[?open");
    expect(stripExplainMarks(PROBE_A)).not.toContain("[?");
    for (const q of PROBE_A_MARKS) expect(stripExplainMarks(PROBE_A)).toContain(q);
  });
});

describe("the desktop chain keeps the span", () => {
  it("emits <span class=\"explain-mark\" data-explain-quote> through sanitize", () => {
    const html = render("旧数据说我把它归因为 [?acquiescence bias]。");
    expect(html).toContain('<span class="explain-mark" data-explain-quote="acquiescence bias">acquiescence bias</span>');
  });

  it("keeps the quote attribute when the mark sits beside emphasis", () => {
    const html = render("**加粗** 然后 [?AUROC 只动了 0.004]。");
    expect(html).toContain("<strong>加粗</strong>");
    expect(html).toContain('data-explain-quote="AUROC 只动了 0.004"');
  });

  it("still strips a raw <span> that fakes the class with a handler", () => {
    const html = render('<span class="explain-mark" onclick="alert(1)" data-explain-quote="x">x</span>');
    expect(html).toContain('class="explain-mark"');
    expect(html).not.toContain("onclick");
  });

  it("renders all five probe-A marks", () => {
    const html = render(PROBE_A);
    expect(html.match(/class="explain-mark"/g)).toHaveLength(5);
    for (const q of PROBE_A_MARKS) expect(html).toContain(`data-explain-quote="${q}"`);
  });
});
