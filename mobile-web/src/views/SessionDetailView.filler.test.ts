import { describe, expect, it } from "vitest";
import type { RawMessage } from "../types";
import { isInterruptMarker, isNoResponseFiller } from "./SessionDetailView";

/** The two filler records a timed-out Decision Card leaves in the transcript,
 *  alongside the near-misses that must keep rendering. */
describe("decision-timeout filler predicates", () => {
  const user = (text: string): RawMessage => ({
    type: "user",
    message: { role: "user", content: [{ type: "text", text }] },
  });
  const synthetic = (text: string): RawMessage => ({
    type: "assistant",
    message: { role: "assistant", model: "<synthetic>", content: [{ type: "text", text }] },
  });

  it("matches both interrupt markers and nothing else", () => {
    expect(isInterruptMarker(user("[Request interrupted by user]"))).toBe(true);
    expect(isInterruptMarker(user("[Request interrupted by user for tool use]"))).toBe(true);
    expect(isInterruptMarker(user("为什么会 [Request interrupted by user]？"))).toBe(false);
    expect(isInterruptMarker(synthetic("[Request interrupted by user]"))).toBe(false);
  });

  it("drops only the no-response filler, never a real <synthetic> record", () => {
    expect(isNoResponseFiller(synthetic("No response requested."))).toBe(true);
    expect(isNoResponseFiller(synthetic("Failed to authenticate. API Error: 403"))).toBe(false);
    // Same text from a real model is a thing the model said.
    const real = synthetic("No response requested.");
    real.message!.model = "claude-opus-5";
    expect(isNoResponseFiller(real)).toBe(false);
  });
});
