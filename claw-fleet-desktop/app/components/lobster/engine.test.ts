import { describe, expect, it } from "vitest";
import { pickScale } from "./engine";

describe("pickScale", () => {
  it("keeps about 220 art pixels of height with an integer scale", () => {
    // 1600x900 @2x → 3200x1800 device px → scale 8 caps at MAX, but 1800/220=8.
    expect(pickScale(3200, 1800)).toEqual({ scale: 8, w: 400, h: 225 });
    // 1280x720 @1x → scale 3, canvas covers the area in whole art pixels.
    expect(pickScale(1280, 720)).toEqual({ scale: 3, w: 426, h: 240 });
  });

  it("never drops below scale 2, even on a tiny stage", () => {
    expect(pickScale(300, 200).scale).toBe(2);
  });

  it("never upscales past 8", () => {
    expect(pickScale(8000, 5000).scale).toBe(8);
  });

  it("keeps the scale a multiple of the detail factor", () => {
    // 1280x720 → 720/(220*2) = 1 fine pixel per device pixel → scale 2.
    expect(pickScale(1280, 720, 220, 2)).toEqual({ scale: 2, w: 640, h: 360 });
    expect(pickScale(1920, 1080, 180, 2).scale).toBe(6);
    expect(pickScale(8000, 5000, 140, 2).scale).toBe(8);
    expect(pickScale(300, 200, 140, 2).scale).toBe(2);
  });

  it("returns a non-empty canvas for a zero-size host", () => {
    const px = pickScale(0, 0);
    expect(px.w).toBeGreaterThan(0);
    expect(px.h).toBeGreaterThan(0);
  });
});
