import { describe, expect, it } from "vitest";

import { pollExplanation, type ExplainRecord } from "./explainApi";

function rec(over: Partial<ExplainRecord>): ExplainRecord {
  return {
    id: "e1",
    sessionId: "s1",
    source: "claude-code",
    createdMs: 1,
    updatedMs: 1,
    preset: "explain",
    quote: "q",
    question: "?",
    status: "running",
    text: "",
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    durationMs: 0,
    ...over,
  };
}

const noSleep = async () => {};

describe("pollExplanation", () => {
  it("reports each change and stops once the record settles", async () => {
    const frames = [
      rec({ text: "", updatedMs: 1 }),
      rec({ text: "", updatedMs: 1 }), // unchanged: not reported twice
      rec({ text: "hel", updatedMs: 2 }),
      rec({ text: "hello", updatedMs: 3, status: "done" }),
      rec({ text: "never read", updatedMs: 4, status: "done" }),
    ];
    let i = 0;
    const seen: string[] = [];
    const out = await pollExplanation(
      async () => frames[i++],
      (r) => seen.push(`${r.status}:${r.text}`),
      { sleep: noSleep },
    );
    expect(seen).toEqual(["running:", "running:hel", "done:hello"]);
    expect(out?.text).toBe("hello");
    expect(i).toBe(4);
  });

  it("treats a failed read as not-yet-written and keeps polling", async () => {
    let i = 0;
    const out = await pollExplanation(
      async () => {
        i += 1;
        if (i < 3) throw new Error("no such explanation");
        return rec({ text: "ok", status: "done" });
      },
      () => {},
      { sleep: noSleep },
    );
    expect(out?.text).toBe("ok");
    expect(i).toBe(3);
  });

  it("stops on abort and hands back the last record seen", async () => {
    const ctl = new AbortController();
    let reads = 0;
    const out = await pollExplanation(
      async () => {
        reads += 1;
        return rec({ text: "partial", updatedMs: reads });
      },
      () => {},
      {
        sleep: async () => {
          if (reads >= 2) ctl.abort();
        },
        signal: ctl.signal,
      },
    );
    expect(out?.text).toBe("partial");
    expect(reads).toBe(2);
  });
});
