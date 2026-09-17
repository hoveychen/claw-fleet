import { describe, expect, it } from "vitest";
import { localDateKey, localDateKeyDaysAgo } from "./localDate";

describe("localDateKey", () => {
  it("本地日界两侧的瞬间不会翻页（UTC 口径会）", () => {
    const justAfterMidnight = new Date(2026, 8, 14, 0, 30, 0);
    const justBeforeMidnight = new Date(2026, 8, 14, 23, 30, 0);
    expect(localDateKey(justAfterMidnight)).toBe("2026-09-14");
    expect(localDateKey(justBeforeMidnight)).toBe("2026-09-14");
    // UTC basis only diverges at one of these two moments if this machine is not in UTC —
    // writing the assertion as "at least one side differs from toISOString" would false-red
    // on CI's UTC machine, so we only pin down the local basis itself here.
  });

  it("月份和日补零", () => {
    const d = new Date(2026, 0, 3, 12, 0, 0);
    expect(localDateKey(d)).toBe("2026-01-03");
  });

  it("跨月回退按本地日历走", () => {
    const d = new Date(2026, 2, 2, 12, 0, 0); // 2026-03-02
    expect(localDateKeyDaysAgo(1, d)).toBe("2026-03-01");
    expect(localDateKeyDaysAgo(2, d)).toBe("2026-02-28");
  });
});
