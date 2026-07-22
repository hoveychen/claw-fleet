import { describe, expect, it } from "vitest";
import { dropWithinRect } from "./ChatComposer";

const rect = { left: 100, top: 100, right: 300, bottom: 200 };

describe("dropWithinRect", () => {
  it("accepts a drop inside the rect at 1× scale", () => {
    expect(dropWithinRect({ x: 150, y: 150 }, rect, 1)).toBe(true);
  });

  it("rejects a drop outside the rect at 1× scale", () => {
    expect(dropWithinRect({ x: 50, y: 150 }, rect, 1)).toBe(false);
    expect(dropWithinRect({ x: 150, y: 250 }, rect, 1)).toBe(false);
  });

  it("converts physical pixels to logical before comparing (2× retina)", () => {
    // Physical (300,300) on a 2× display is logical (150,150) → inside.
    expect(dropWithinRect({ x: 300, y: 300 }, rect, 2)).toBe(true);
    // Physical (150,150) on 2× is logical (75,75) → left of the rect.
    expect(dropWithinRect({ x: 150, y: 150 }, rect, 2)).toBe(false);
  });

  it("accepts drops exactly on the edges", () => {
    expect(dropWithinRect({ x: 100, y: 100 }, rect, 1)).toBe(true);
    expect(dropWithinRect({ x: 300, y: 200 }, rect, 1)).toBe(true);
  });

  it("treats a zero/invalid dpr as 1× rather than dividing by zero", () => {
    expect(dropWithinRect({ x: 150, y: 150 }, rect, 0)).toBe(true);
  });
});
