import { describe, expect, it } from "vitest";
import { fmtBadgeCount } from "./railNumbers";

describe("fmtBadgeCount", () => {
  it("shows exact counts up to 99", () => {
    expect(fmtBadgeCount(0)).toBe("0");
    expect(fmtBadgeCount(99)).toBe("99");
  });

  it("caps beyond 99 so the pill cannot cover the rail icon", () => {
    // The badge in the screenshot read 1881 and swallowed the shield icon.
    expect(fmtBadgeCount(100)).toBe("99+");
    expect(fmtBadgeCount(1881)).toBe("99+");
  });

  it("stays within 3 glyphs", () => {
    for (const v of [0, 7, 99, 100, 1881, 999999]) {
      expect(fmtBadgeCount(v).length).toBeLessThanOrEqual(3);
    }
  });
});
