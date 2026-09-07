import { describe, expect, it } from "vitest";

import { followGrowthBehavior, liveThinkingLanded, retainLiveThinking } from "./streamContinuity";
import type { LiveThinking, RawMessage } from "./types";

const snapshot = (thinking: string, streaming = true): LiveThinking => ({
  sessionId: "session-1",
  thinking,
  streaming,
  updatedSecsAgo: 0,
});

describe("stream continuity", () => {
  it("keeps the last visible reasoning through an empty in-flight snapshot", () => {
    const previous = snapshot("already visible");
    expect(retainLiveThinking(previous, snapshot(""))).toBe(previous);
    expect(retainLiveThinking(previous, null)).toBe(previous);
  });

  it("accepts new visible reasoning and a terminal snapshot", () => {
    const previous = snapshot("old");
    const next = snapshot("new");
    const terminal = snapshot("new", false);
    expect(retainLiveThinking(previous, next)).toBe(next);
    expect(retainLiveThinking(previous, terminal)).toBe(terminal);
  });

  it("hands live reasoning off only after it lands in the transcript", () => {
    const live = snapshot("visible reasoning");
    const landed: RawMessage = {
      type: "assistant",
      message: { role: "assistant", content: [{ type: "thinking", thinking: "visible reasoning completed" }] },
    };
    expect(liveThinkingLanded([], live)).toBe(false);
    expect(liveThinkingLanded([landed], live)).toBe(true);
  });

  it("animates content growth after the initial pin", () => {
    expect(followGrowthBehavior(null, 500)).toBe("instant");
    expect(followGrowthBehavior(500, 540)).toBe("smooth");
    expect(followGrowthBehavior(540, 540)).toBeNull();
    expect(followGrowthBehavior(540, 520)).toBe("instant");
  });
});
