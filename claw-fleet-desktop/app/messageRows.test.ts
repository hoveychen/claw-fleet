import { describe, it, expect } from "vitest";
import { shortModelName, turnUsageByRow } from "./messageRows";
import type { RawMessage } from "./types";

describe("shortModelName", () => {
  it("compacts claude model ids", () => {
    expect(shortModelName("claude-opus-4-8")).toBe("opus 4.8");
    expect(shortModelName("claude-sonnet-5")).toBe("sonnet 5");
    expect(shortModelName("claude-fable-5")).toBe("fable 5");
    expect(shortModelName("claude-fable-5-1")).toBe("fable 5.1");
    expect(shortModelName("claude-haiku-4-5-20251001")).toBe("haiku 4.5");
  });

  it("passes non-claude ids through unchanged", () => {
    expect(shortModelName("gpt-5.6-sol")).toBe("gpt-5.6-sol");
    expect(shortModelName("gpt-5.5")).toBe("gpt-5.5");
  });
});

function assistant(inTok: number, outTok: number): RawMessage {
  return {
    type: "assistant",
    message: {
      role: "assistant",
      content: [],
      usage: { input_tokens: inTok, output_tokens: outTok },
    },
  };
}

function user(): RawMessage {
  return { type: "user", message: { role: "user", content: "hi" } };
}

describe("turnUsageByRow", () => {
  it("keys one aggregate on each turn's final assistant row", () => {
    const rows = [user(), assistant(100, 10), assistant(120, 20), user(), assistant(200, 5)];
    const map = turnUsageByRow(rows);
    expect([...map.keys()]).toEqual([2, 4]);
    expect(map.get(2)).toEqual({ inputTokens: 120, outputTokens: 30 });
    expect(map.get(4)).toEqual({ inputTokens: 200, outputTokens: 5 });
  });

  it("skips records without usage but keeps the turn total honest", () => {
    const noUsage: RawMessage = {
      type: "assistant",
      message: { role: "assistant", content: [] },
    };
    const rows = [assistant(100, 10), noUsage];
    const map = turnUsageByRow(rows);
    // The turn ends on a usage-less record; the aggregate still lands there.
    expect(map.get(1)).toEqual({ inputTokens: 100, outputTokens: 10 });
  });

  it("emits nothing for turns with no usage at all", () => {
    const noUsage: RawMessage = {
      type: "assistant",
      message: { role: "assistant", content: [] },
    };
    expect(turnUsageByRow([noUsage]).size).toBe(0);
  });
});
