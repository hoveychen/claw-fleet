import { describe, expect, it } from "vitest";
import { keyboardInset } from "./keyboardInset";

describe("keyboardInset", () => {
  it("is zero with no keyboard", () => {
    expect(keyboardInset(714, 714, 0)).toBe(0);
  });

  it("is the hidden strip below the visible area", () => {
    expect(keyboardInset(714, 378, 0)).toBe(336);
  });

  it("subtracts the iOS pan so a fixed bottom lands on the visible bottom", () => {
    expect(keyboardInset(714, 378, 120)).toBe(216);
  });

  it("ignores toolbar wobble that is not a keyboard", () => {
    expect(keyboardInset(714, 690, 0)).toBe(0);
  });
});
