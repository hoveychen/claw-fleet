import { describe, expect, it } from "vitest";
import { tailMayHaveEarlier } from "./SessionDetailView";

describe("tailMayHaveEarlier", () => {
  it("a short reply reached the start of the transcript", () => {
    expect(tailMayHaveEarlier(87, 120)).toBe(false);
    expect(tailMayHaveEarlier(0, 120)).toBe(false);
  });

  it("a full reply may have stopped short of the start", () => {
    expect(tailMayHaveEarlier(120, 120)).toBe(true);
  });

  it("a widened window that comes back short hides the button again", () => {
    expect(tailMayHaveEarlier(250, 320)).toBe(false);
  });
});
