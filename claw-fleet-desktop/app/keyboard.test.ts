import { describe, expect, it } from "vitest";
import { isKeyboardActivationKey } from "./keyboard";

describe("isKeyboardActivationKey", () => {
  it("accepts the two standard button activation keys", () => {
    expect(isKeyboardActivationKey("Enter")).toBe(true);
    expect(isKeyboardActivationKey(" ")).toBe(true);
  });

  it("does not treat navigation keys as activation", () => {
    expect(isKeyboardActivationKey("ArrowRight")).toBe(false);
    expect(isKeyboardActivationKey("Escape")).toBe(false);
  });
});
