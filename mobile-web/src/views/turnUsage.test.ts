import { describe, expect, it } from "vitest";
import { fmtTokens, shortModelName, turnUsageByIndex } from "./turnUsage";
import type { RawMessage } from "../types";

const assistant = (out: number, input: number, model?: string): RawMessage => ({
  type: "assistant",
  message: {
    role: "assistant",
    model,
    usage: { input_tokens: input, output_tokens: out },
    content: [],
  },
});

const user = (): RawMessage => ({ type: "user", message: { role: "user", content: "hi" } });

describe("turnUsageByIndex", () => {
  it("keys one aggregated entry on each turn's final record", () => {
    const rows = [
      user(),
      assistant(100, 5000, "claude-opus-4-8"),
      assistant(200, 5200, "claude-opus-4-8"),
      user(),
      assistant(50, 6000, "claude-sonnet-5"),
    ];
    const map = turnUsageByIndex(rows);
    expect(map.size).toBe(2);
    // First turn ends at index 2: outputs summed, input = last record's.
    expect(map.get(2)).toEqual({ inputTokens: 5200, outputTokens: 300, model: "claude-opus-4-8" });
    expect(map.get(4)).toEqual({ inputTokens: 6000, outputTokens: 50, model: "claude-sonnet-5" });
    expect(map.get(1)).toBeUndefined();
  });

  it("yields no entry when the server strips usage (old relay)", () => {
    const rows: RawMessage[] = [
      { type: "assistant", message: { role: "assistant", content: [] } },
    ];
    expect(turnUsageByIndex(rows).size).toBe(0);
  });

  it("closes a trailing run at the end of the list", () => {
    const rows = [user(), assistant(10, 100)];
    expect(turnUsageByIndex(rows).get(1)).toMatchObject({ outputTokens: 10 });
  });
});

describe("shortModelName", () => {
  it("shortens known families and passes unknowns through", () => {
    expect(shortModelName("claude-opus-4-8")).toBe("opus");
    expect(shortModelName("claude-fable-5")).toBe("fable");
    expect(shortModelName("claude-fable-5-1")).toBe("fable");
    expect(shortModelName("gpt-5.6-sol")).toBe("sol");
    expect(shortModelName("weird-model")).toBe("weird-model");
    expect(shortModelName(undefined)).toBeUndefined();
  });
});

describe("fmtTokens", () => {
  it("formats compactly", () => {
    expect(fmtTokens(812)).toBe("812");
    expect(fmtTokens(4832)).toBe("4.8k");
    expect(fmtTokens(123456)).toBe("123k");
  });
});
