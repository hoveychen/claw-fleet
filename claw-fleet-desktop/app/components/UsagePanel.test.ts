import { describe, expect, it } from "vitest";
import { popoverBottom } from "./UsagePanel";

describe("popoverBottom", () => {
  it("aligns with the tile's bottom edge when the panel fits above it", () => {
    // Tile bottom at 800 in a 1280px window, 300px panel: top lands at 500.
    expect(popoverBottom(800, 300, 1280)).toBe(480);
  });

  it("slides down so a tall panel keeps its top inside the window", () => {
    // Bottom-aligned, a 1000px panel would start at -200.
    const bottom = popoverBottom(800, 1000, 1280);
    expect(1280 - bottom - 1000).toBe(8);
  });

  it("keeps the bottom margin when the panel fills the window", () => {
    expect(popoverBottom(800, 1264, 1280)).toBe(8);
  });

  it("does not sit below the window's bottom margin for a tile at the bottom", () => {
    expect(popoverBottom(1279, 100, 1280)).toBe(8);
  });
});
