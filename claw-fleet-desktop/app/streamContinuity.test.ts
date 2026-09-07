import { describe, expect, it } from "vitest";

import { followGrowthBehavior, liveThinkingLanded, retainLiveThinking } from "./streamContinuity";
import type { LiveThinking, RawMessage } from "./types";

const snapshot = (thinking: string, streaming = true, sessionId = "session-1"): LiveThinking => ({
  sessionId,
  thinking,
  streaming,
  updatedSecsAgo: 0,
});

describe("stream continuity", () => {
  it("keeps the last visible reasoning through an empty in-flight snapshot", () => {
    const previous = snapshot("already visible");
    expect(retainLiveThinking(previous, snapshot(""), "session-1")).toBe(previous);
    expect(retainLiveThinking(previous, null, "session-1")).toBe(previous);
  });

  it("accepts new visible reasoning and a terminal snapshot", () => {
    const previous = snapshot("old");
    const next = snapshot("new");
    const terminal = snapshot("new", false);
    expect(retainLiveThinking(previous, next, "session-1")).toBe(next);
    expect(retainLiveThinking(previous, terminal, "session-1")).toBe(terminal);
  });

  // The retain-through-empty rule above is what let one session's reasoning
  // stay pinned above another's composer: SessionDetail is not remounted on
  // session switch, and a session with no sidecar at all polls back exactly
  // like a session mid-empty-sample. The session id is the discriminator.
  it("drops retained reasoning belonging to a different session", () => {
    const previous = snapshot("session A reasoning", true, "session-a");
    expect(retainLiveThinking(previous, null, "session-b")).toBeNull();
    expect(retainLiveThinking(previous, snapshot("", true, "session-b"), "session-b")).toBeNull();
  });

  it("ignores a snapshot that arrived for a session we are no longer showing", () => {
    const previous = snapshot("session B reasoning", true, "session-b");
    const stale = snapshot("session A reasoning", true, "session-a");
    expect(retainLiveThinking(previous, stale, "session-b")).toBe(previous);
    expect(retainLiveThinking(null, stale, "session-b")).toBeNull();
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
