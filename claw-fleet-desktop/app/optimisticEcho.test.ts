import { describe, it, expect } from "vitest";
import { landedUserTexts, stillPending } from "./optimisticEcho";
import type { RawMessage } from "./types";

function user(text: string, isMeta?: boolean): RawMessage {
  return {
    type: "user",
    timestamp: "2026-09-07T01:30:00.000Z",
    message: { role: "user", content: [{ type: "text", text }] },
    ...(isMeta ? { isMeta: true } : {}),
  } as RawMessage;
}

describe("landedUserTexts", () => {
  it("counts a real user bubble as landed", () => {
    const landed = landedUserTexts([user("修一下花费明细")]);
    expect(stillPending("修一下花费明细", landed)).toBe(false);
  });

  it("ignores leading and trailing whitespace on both sides", () => {
    const landed = landedUserTexts([user("  继续  ")]);
    expect(stillPending("继续\n", landed)).toBe(false);
  });

  // dsh stores its agent-instructions, its runtime snapshot and every Fleet
  // guidance block as `user/message` records; core flags them `isMeta` and the
  // list folds them into one 系统上下文 card. Counting one as the user's prompt
  // retired the echo against a row nobody can see, and the submitted message
  // disappeared from the conversation with nothing left in its place.
  it("does not count a folded meta row as the user's prompt", () => {
    const landed = landedUserTexts([user("修一下花费明细", true)]);
    expect(stillPending("修一下花费明细", landed)).toBe(true);
  });

  it("ignores assistant rows", () => {
    const assistant: RawMessage = {
      type: "assistant",
      timestamp: "2026-09-07T01:30:01.000Z",
      message: { role: "assistant", content: [{ type: "text", text: "继续" }] },
    } as RawMessage;
    expect(stillPending("继续", landedUserTexts([assistant]))).toBe(true);
  });
});
