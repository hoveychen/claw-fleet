import { describe, expect, it } from "vitest";
import { mergeAttention } from "./AttentionView";
import type { DailyAttention, DriftCheck } from "../types";

const drift = (chainId: string, checkedAt: number, question = ""): DriftCheck => ({
  chainId,
  workspacePath: "/w",
  workspaceName: "w",
  planId: null,
  goal: "g",
  sessionCount: 2,
  latestSessionId: `s-${chainId}`,
  verdict: "polishing",
  evidence: "",
  question,
  checkedAt,
});

const lesson = (content: string) => ({
  content,
  reason: "r",
  workspaceName: "w",
  sessionId: "s",
  evidenceSessionIds: ["a", "b"],
});

describe("mergeAttention", () => {
  it("keeps the newest drift check per chain, newest first", () => {
    const today: DailyAttention = {
      date: "2026-10-03",
      drift: [drift("c1", 300, "new"), drift("c2", 100)],
      lessons: [],
      violations: [],
    };
    const yesterday: DailyAttention = {
      date: "2026-10-02",
      drift: [drift("c1", 200, "old"), drift("c3", 250)],
      lessons: [],
      violations: [],
    };
    const { drift: out } = mergeAttention([yesterday, today]);
    expect(out.map((d) => d.chainId)).toEqual(["c1", "c3", "c2"]);
    expect(out[0].question).toBe("new");
  });

  it("folds duplicate lessons by content and violations by lesson id", () => {
    const v = { lessonId: "l1", lessonContent: "x", sessionIds: ["a"], note: "" };
    const day = (date: string): DailyAttention => ({
      date,
      drift: [],
      lessons: [lesson("same"), lesson(`only-${date}`)],
      violations: [v],
    });
    const out = mergeAttention([day("2026-10-03"), day("2026-10-02")]);
    expect(out.lessons.map((l) => l.content)).toEqual(["same", "only-2026-10-03", "only-2026-10-02"]);
    expect(out.violations).toHaveLength(1);
  });

  it("tolerates days with missing arrays", () => {
    const out = mergeAttention([{ date: "2026-10-03" } as DailyAttention]);
    expect(out).toEqual({ drift: [], lessons: [], violations: [] });
  });
});
