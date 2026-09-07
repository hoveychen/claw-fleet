import { describe, expect, it } from "vitest";
import { fmtBadgeCount, fmtRailCount, fmtRailMoney } from "./railNumbers";

/** Widest string any of these may produce, in glyphs. A rail tile has ~46px of
 *  inner width at 13px tabular digits ≈ 6 glyphs. */
const MAX_GLYPHS = 6;

describe("fmtRailMoney", () => {
  it("keeps cents for小额", () => {
    expect(fmtRailMoney(0)).toBe("$0.00");
    expect(fmtRailMoney(6.62)).toBe("$6.62");
    expect(fmtRailMoney(99.994)).toBe("$99.99");
  });

  it("drops cents once三位数", () => {
    expect(fmtRailMoney(100)).toBe("$100");
    expect(fmtRailMoney(312.4)).toBe("$312");
  });

  it("compacts thousands so the tile still fits", () => {
    // The overflow that started this: $3165.41 spilled out of the tile.
    expect(fmtRailMoney(3165.41)).toBe("$3.2k");
    expect(fmtRailMoney(1000)).toBe("$1.0k");
    expect(fmtRailMoney(12345)).toBe("$12k");
    expect(fmtRailMoney(999999)).toBe("$1000k");
  });

  it("never exceeds the tile budget for plausible spend", () => {
    for (const v of [0, 0.004, 9.99, 87.5, 340, 3165.41, 42000]) {
      expect(fmtRailMoney(v).length).toBeLessThanOrEqual(MAX_GLYPHS);
    }
  });

  it("survives negatives and非有限值", () => {
    expect(fmtRailMoney(-2.5)).toBe("-$2.50");
    expect(fmtRailMoney(Number.NaN)).toBe("$0.00");
  });
});

describe("fmtRailCount", () => {
  it("shows small counts verbatim", () => {
    expect(fmtRailCount(0)).toBe("0");
    expect(fmtRailCount(215)).toBe("215");
  });

  it("compacts thousands", () => {
    expect(fmtRailCount(1200)).toBe("1.2k");
    expect(fmtRailCount(34000)).toBe("34k");
  });

  it("clamps负数与非有限值", () => {
    expect(fmtRailCount(-5)).toBe("0");
    expect(fmtRailCount(Number.NaN)).toBe("0");
  });
});

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
