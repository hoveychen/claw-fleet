import { describe, expect, it } from "vitest";
import { decisionTerminalOutcome, parseAnswersFromResultText } from "./decisionText";
import type { DecisionHistoryRecord } from "./types";

/** Minimal record for the outcome-reading path; only `kind`+`outcome` matter. */
function rec(
  kind: "elicitation" | "fleet-ask" | "plan-approval",
  outcome: string,
): DecisionHistoryRecord {
  return { kind, outcome } as unknown as DecisionHistoryRecord;
}

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

describe("decisionTerminalOutcome", () => {
  it("reports a declined elicitation so it is not stuck 未回答", () => {
    expect(decisionTerminalOutcome(rec("elicitation", "declined"), false)).toBe("declined");
  });

  it("reports timeout / cancelled / heartbeat-lost", () => {
    expect(decisionTerminalOutcome(rec("elicitation", "timeout"), false)).toBe("timeout");
    expect(decisionTerminalOutcome(rec("fleet-ask", "cancelled"), false)).toBe("cancelled");
    expect(decisionTerminalOutcome(rec("fleet-ask", "heartbeat-lost"), false)).toBe(
      "heartbeat-lost",
    );
  });

  it("returns null for an answered record", () => {
    expect(decisionTerminalOutcome(rec("elicitation", "answered"), false)).toBeNull();
  });

  it("returns null when the card actually has an answer, whatever the record says", () => {
    // Defensive: an answer in hand always wins over a stale record outcome.
    expect(decisionTerminalOutcome(rec("elicitation", "declined"), true)).toBeNull();
  });

  it("returns null with no record (still pending) or a non-question record", () => {
    expect(decisionTerminalOutcome(null, false)).toBeNull();
    expect(decisionTerminalOutcome(undefined, false)).toBeNull();
    expect(decisionTerminalOutcome(rec("plan-approval", "timeout"), false)).toBeNull();
  });
});
