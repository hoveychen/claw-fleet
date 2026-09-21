// A follow-up is a *new* record that threads off the one it continues (the
// fork is never resumed). Rendered as flat newest-first records, a two-turn
// chain reads backwards and gets split apart by anything asked in between —
// so the column groups by chain instead.
import { describe, expect, it } from "vitest";
import { groupExplainThreads, threadRootId } from "./selectionExplain";
import type { ExplainRecord } from "./explainApi";

function rec(id: string, createdMs: number, thread: string[] = []): ExplainRecord {
  return {
    id,
    sessionId: "sess-1",
    source: "claude-code",
    createdMs,
    updatedMs: createdMs,
    preset: thread.length > 0 ? "custom" : "explain",
    quote: "灰度到 5%",
    question: `q-${id}`,
    thread,
    status: "done",
    text: `a-${id}`,
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    durationMs: 0,
  };
}

describe("threadRootId", () => {
  it("is the record itself for a first question", () => {
    expect(threadRootId(rec("a", 1))).toBe("a");
  });

  it("is the chain's oldest id for a follow-up, not its immediate parent", () => {
    expect(threadRootId(rec("c", 3, ["a", "b"]))).toBe("a");
  });
});

describe("groupExplainThreads", () => {
  it("keeps a chain together and in asking order", () => {
    const threads = groupExplainThreads([rec("a", 1), rec("b", 2, ["a"]), rec("c", 3, ["a", "b"])]);
    expect(threads).toHaveLength(1);
    expect(threads[0].id).toBe("a");
    expect(threads[0].records.map((r) => r.id)).toEqual(["a", "b", "c"]);
  });

  it("orders chains newest-first", () => {
    const threads = groupExplainThreads([rec("a", 1), rec("x", 5)]);
    expect(threads.map((t) => t.id)).toEqual(["x", "a"]);
  });

  it("pulls a chain back to the top when it gets a follow-up", () => {
    // 'a' was asked first, 'x' second, then 'a' was followed up on.
    const threads = groupExplainThreads([rec("a", 1), rec("x", 5), rec("b", 9, ["a"])]);
    expect(threads.map((t) => t.id)).toEqual(["a", "x"]);
    expect(threads[0].records.map((r) => r.id)).toEqual(["a", "b"]);
  });

  it("does not merge unrelated questions about the same passage", () => {
    const threads = groupExplainThreads([rec("a", 1), rec("b", 2)]);
    expect(threads.map((t) => t.id)).toEqual(["b", "a"]);
  });

  it("keeps a follow-up whose root was dismissed", () => {
    const threads = groupExplainThreads([rec("b", 2, ["a"])]);
    expect(threads.map((t) => t.id)).toEqual(["a"]);
    expect(threads[0].records.map((r) => r.id)).toEqual(["b"]);
  });

  it("leaves the input array untouched", () => {
    const input = [rec("a", 1), rec("x", 5)];
    groupExplainThreads(input);
    expect(input.map((r) => r.id)).toEqual(["a", "x"]);
  });
});
