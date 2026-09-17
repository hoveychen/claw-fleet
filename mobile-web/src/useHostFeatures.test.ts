import { describe, expect, it } from "vitest";
import { normalizeHostFeatures } from "./useHostFeatures";

/** The phone's terminal entry is gated solely by this flag, and its default
 *  must be off: an older desktop that does not recognize the `host_features`
 *  method (returns null / errors), or whose response lacks this field, must
 *  not be read as enabled — that would let users navigate to the terminal page
 *  only to have their first shell command rejected, revealing the feature was
 *  never enabled. */
describe("normalizeHostFeatures", () => {
  it("treats a proper affirmative as on", () => {
    expect(normalizeHostFeatures({ terminal: true })).toEqual({ terminal: true });
  });

  it.each([
    ["explicit false", { terminal: false }],
    ["missing field", {}],
    ["null answer", null],
    ["undefined answer", undefined],
    ["a truthy string, not a boolean", { terminal: "true" }],
    ["a truthy number, not a boolean", { terminal: 1 }],
    ["nonsense", "yes"],
  ])("fails closed on %s", (_name, raw) => {
    expect(normalizeHostFeatures(raw)).toEqual({ terminal: false });
  });
});
