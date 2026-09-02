// The model label on a session card. `formatModel` has two regex branches —
// a two-part version (`claude-opus-4-8` → "Opus 4.8") and a single-version
// family (`claude-fable-5` → "Fable 5") — and a new model id lands in whichever
// one its shape matches, with no registry to add it to. `claude-fable-5-1` is
// the first Fable that falls in the *two-part* branch, so it is asserted here:
// if the single-version branch ever became greedier, the card would silently
// print the raw id instead of "Fable 5.1".
import { describe, expect, it } from "vitest";

import { formatModel } from "./SessionCard";

describe("formatModel", () => {
  it("renders two-part version ids", () => {
    expect(formatModel("claude-opus-4-8")).toBe("Opus 4.8");
    expect(formatModel("claude-sonnet-4-6")).toBe("Sonnet 4.6");
    expect(formatModel("claude-haiku-4-5-20251001")).toBe("Haiku 4.5");
    expect(formatModel("claude-fable-5-1")).toBe("Fable 5.1");
  });

  it("renders single-version family ids", () => {
    expect(formatModel("claude-fable-5")).toBe("Fable 5");
    expect(formatModel("claude-opus-5")).toBe("Opus 5");
    expect(formatModel("claude-mythos-5")).toBe("Mythos 5");
  });

  it("passes unrecognised ids through unchanged", () => {
    expect(formatModel("gpt-5.6-sol")).toBe("gpt-5.6-sol");
  });
});
