import { describe, expect, it } from "vitest";
import { parseAnswersFromResultText } from "./decisionText";

const question = {
  question: "根因是模型漏遵守规则。\n---\nBoss，接下来怎么处理？",
  options: [{ label: "修复两个缺口 (Recommended)" }],
};

describe("parseAnswersFromResultText", () => {
  it("parses the bare JSON answer map returned by Codex fleet__ask", () => {
    const text = JSON.stringify({
      [question.question]: "修复两个缺口 (Recommended)",
    });

    expect(parseAnswersFromResultText(text, [question])).toEqual({
      [question.question]: "修复两个缺口 (Recommended)",
    });
  });

  it("keeps parsing Claude's legacy prose result", () => {
    const text = `Your questions have been answered: "${question.question}"="修复两个缺口 (Recommended)". You can now continue with these answers in mind.`;

    expect(parseAnswersFromResultText(text, [question])).toEqual({
      [question.question]: "修复两个缺口 (Recommended)",
    });
  });

  it("ignores JSON keys that are not known questions", () => {
    expect(parseAnswersFromResultText('{"unexpected":"secret"}', [question])).toEqual({});
  });
});
