import { describe, expect, it } from "vitest";
import { inFlightToolIds, isBackgroundShell } from "./inFlightTools";
import type { ContentBlock, RawMessage } from "../types";

const blocksOf = (m: RawMessage): ContentBlock[] =>
  Array.isArray(m.message?.content) ? (m.message!.content as ContentBlock[]) : [];

function assistant(stopReason: string | null, ids: string[]): RawMessage {
  return {
    type: "assistant",
    message: {
      stop_reason: stopReason,
      content: ids.map((id) => ({ type: "tool_use", id, name: "Bash", input: { command: "sleep 600" } })),
    },
  } as unknown as RawMessage;
}

describe("inFlightToolIds", () => {
  it("claims a finalised tool_use whose result has not landed", () => {
    // stop_reason is already "tool_use" — nothing is streaming, yet the tool is
    // still executing. That window is the whole bug.
    const ids = inFlightToolIds([assistant("tool_use", ["t1"])], new Set(), true, blocksOf);
    expect([...ids]).toEqual(["t1"]);
  });

  it("stays empty when the session is not in a working status", () => {
    // A killed turn leaves the same shape behind forever.
    expect(inFlightToolIds([assistant("tool_use", ["t1"])], new Set(), false, blocksOf).size).toBe(0);
  });

  it("ignores calls whose result is already back", () => {
    const ids = inFlightToolIds(
      [assistant("tool_use", ["t1", "t2"])],
      new Set(["t1"]),
      true,
      blocksOf,
    );
    expect([...ids]).toEqual(["t2"]);
  });

  it("only reads the newest assistant record", () => {
    // An older call missing its result was trimmed by the snapshot, not live.
    const msgs = [assistant("tool_use", ["old"]), assistant("end_turn", [])];
    expect(inFlightToolIds(msgs, new Set(), true, blocksOf).size).toBe(0);
  });
});

describe("isBackgroundShell", () => {
  it("only claims a Bash launched with run_in_background", () => {
    expect(isBackgroundShell({ type: "tool_use", name: "Bash", input: { run_in_background: true } })).toBe(true);
    expect(isBackgroundShell({ type: "tool_use", name: "Bash", input: { command: "ls" } })).toBe(false);
    expect(isBackgroundShell({ type: "tool_use", name: "Read", input: { run_in_background: true } })).toBe(false);
  });
});
