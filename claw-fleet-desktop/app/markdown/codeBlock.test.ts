import { describe, it, expect } from "vitest";
import { isFencedBlock } from "./codeBlock";

describe("isFencedBlock", () => {
  it("treats a fence with a language info-string as a block", () => {
    expect(isFencedBlock("language-ts", "const x = 1")).toBe(true);
  });

  it("treats a no-language fence spanning multiple lines as a block", () => {
    // The regression: an ASCII diagram in a ``` fence with no language tag.
    // react-markdown v10 dropped the `inline` prop, so the multi-line signal
    // is what keeps this out of the inline-code branch.
    const diagram = "┌────┐\n│ hi │\n└────┘";
    expect(isFencedBlock(undefined, diagram)).toBe(true);
    expect(isFencedBlock("", diagram)).toBe(true);
  });

  it("treats a single-token inline span as inline (not a block)", () => {
    expect(isFencedBlock(undefined, "someVar")).toBe(false);
    expect(isFencedBlock("", "path/to/file.ts")).toBe(false);
  });
});
