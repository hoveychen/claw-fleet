import { describe, expect, it } from "vitest";
import { effortChipLabel, effortTitle } from "./components/SessionCard";

// The chip used to read `thinkingLevel`, a slot that carried BOTH "this
// transcript has thinking blocks" and "this session runs at high" — which is
// why every reader had to filter magic strings. These pin the split.
describe("effortChipLabel", () => {
  it("names the effort dial when the harness reported one", () => {
    expect(effortChipLabel({ effort: "high", thinkingLevel: "thinking" })).toBe("high");
    // `medium` is a real answer, not an absence — the detail header shows it.
    expect(effortChipLabel({ effort: "medium", thinkingLevel: null })).toBe("medium");
  });

  it("never shows the bare extended-thinking marker as if it were a level", () => {
    expect(effortChipLabel({ effort: null, thinkingLevel: "thinking" })).toBeNull();
  });

  it("still honours a subagent's declared level from its meta.json", () => {
    expect(effortChipLabel({ effort: null, thinkingLevel: "max" })).toBe("max");
  });

  it("no chip at all when neither is known", () => {
    expect(effortChipLabel({ effort: null, thinkingLevel: null })).toBeNull();
  });
});

describe("effortTitle", () => {
  const t = (k: string, o?: Record<string, unknown>) => `${k}:${o?.level}`;

  it("calls the effort dial an effort and the legacy marker extended thinking", () => {
    expect(effortTitle(t, { effort: "high", thinkingLevel: null })).toBe("card.tip_effort:high");
    expect(effortTitle(t, { effort: null, thinkingLevel: "max" })).toBe("card.tip_thinking:max");
  });
});
