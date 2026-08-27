import { describe, it, expect } from "vitest";
import {
  normalizeAnswer,
  readDecisionQuestions,
  resolveAnswers,
  selectedLabels,
  stripTtsDivider,
  summarizeQuestion,
} from "./decisionCall";

const INPUT = {
  questions: [
    {
      question: "查清楚了。\n\n---\n\n正文……\n\n按哪个范围做？",
      header: "渲染范围",
      multiSelect: false,
      options: [
        { label: "展开体 + chip", description: "推荐" },
        { label: "只做展开体", description: "最小改动", preview: "…" },
        { junk: true },
      ],
      formFields: [{ name: "note", kind: "textarea", label: "备注", required: false }],
    },
    { question: "第二题", header: "其二", multiSelect: true, options: [] },
    { header: "无题" },
  ],
};

describe("readDecisionQuestions", () => {
  it("reads questions, drops malformed entries and options", () => {
    const qs = readDecisionQuestions(INPUT);
    expect(qs.map((q) => q.header)).toEqual(["渲染范围", "其二"]);
    expect(qs[0].options?.map((o) => o.label)).toEqual(["展开体 + chip", "只做展开体"]);
    expect(qs[0].options?.[1].preview).toBe("…");
    expect(qs[0].formFields?.[0].name).toBe("note");
    expect(qs[1].multiSelect).toBe(true);
  });

  it("returns [] for an input the card cannot render", () => {
    expect(readDecisionQuestions({})).toEqual([]);
    expect(readDecisionQuestions({ questions: "nope" })).toEqual([]);
  });
});

describe("resolveAnswers", () => {
  const qs = readDecisionQuestions(INPUT);
  const first = qs[0].question;

  it("reads the JSON string toolUseResult an MCP fleet__ask returns", () => {
    const meta = JSON.stringify({ answers: { [first]: "展开体 + chip" } });
    expect(resolveAnswers(meta, "", qs)).toEqual({ [first]: "展开体 + chip" });
  });

  it("reads the object toolUseResult AskUserQuestion returns", () => {
    const meta = { questions: [], answers: { [first]: "只做展开体", note: "写点备注" } };
    expect(resolveAnswers(meta, "", qs)).toEqual({
      [first]: "只做展开体",
      note: "写点备注",
    });
  });

  it("falls back to the result content when toolUseResult is missing", () => {
    const content = JSON.stringify({ answers: { [first]: "展开体 + chip" } });
    expect(resolveAnswers(undefined, content, qs)).toEqual({ [first]: "展开体 + chip" });
  });

  it("parses the AskUserQuestion prose format as a last resort", () => {
    const content =
      `Your questions have been answered: "${first}"="只做展开体". ` +
      `You can now continue with these answers in mind.`;
    expect(resolveAnswers(null, content, qs)).toEqual({ [first]: "只做展开体" });
  });

  it("only copies keys belonging to this card's questions", () => {
    const content = JSON.stringify({ [first]: "只做展开体", 别的问题: "别的答案" });
    expect(resolveAnswers(null, content, qs)).toEqual({ [first]: "只做展开体" });
  });

  it("yields nothing for an unparseable result", () => {
    expect(resolveAnswers(null, "Error: interrupted", qs)).toEqual({});
  });
});

describe("normalizeAnswer / selectedLabels", () => {
  const labels = ["展开体 + chip", "只做展开体"];

  it("marks a matching label as chosen, not free text", () => {
    const a = normalizeAnswer("只做展开体", labels)!;
    expect(a.other).toBe(false);
    expect(selectedLabels(a)).toEqual(new Set(["只做展开体"]));
  });

  it("splits a multi-select answer into every chosen label", () => {
    const a = normalizeAnswer("展开体 + chip, 只做展开体", labels)!;
    expect(selectedLabels(a)).toEqual(new Set(labels));
  });

  it("treats an unmatched answer as free text and marks nothing", () => {
    const a = normalizeAnswer("自己写的回答", labels)!;
    expect(a.other).toBe(true);
    expect(selectedLabels(a).size).toBe(0);
  });

  it("peels @path attachments off the label", () => {
    const a = normalizeAnswer("只做展开体 @/tmp/a.png", labels)!;
    expect(a.label).toBe("只做展开体");
    expect(a.attachments).toEqual(["/tmp/a.png"]);
    expect(a.other).toBe(false);
  });

  it("is null for an unanswered question", () => {
    expect(normalizeAnswer(undefined, labels)).toBeNull();
    expect(normalizeAnswer("   ", labels)).toBeNull();
  });
});

describe("stripTtsDivider / summarizeQuestion", () => {
  const q = INPUT.questions[0].question!;

  it("drops the divider line but keeps the whole body", () => {
    const body = stripTtsDivider(q);
    expect(body).toContain("查清楚了。");
    expect(body).toContain("按哪个范围做？");
    expect(body.split("\n").some((l) => l.trim() === "---")).toBe(false);
  });

  it("summarizes to the line above the divider", () => {
    expect(summarizeQuestion(q)).toBe("查清楚了。");
  });

  it("caps a long divider-less question", () => {
    expect(summarizeQuestion("啊".repeat(200))).toHaveLength(80);
  });
});
