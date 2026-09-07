import { describe, expect, it } from "vitest";
import { codexRateLimitBars, codexRateLimitLabel, type TFunc } from "./codexUsage";
import { barsForPoint } from "./components/CodexUsageHistoryChart";

const t: TFunc = (key, opts) => opts?.n == null ? key : `${key}:${opts.n}`;

describe("Codex dynamic usage buckets", () => {
  it("prefers the provider limit name and includes the real duration", () => {
    expect(codexRateLimitLabel(
      { limitId: "base_model_inference", limitName: "Luna Reserve", normalModelSlug: "gpt-reserve" },
      { usedPercent: 48, windowDurationMins: 10080 },
      t,
    )).toBe("Luna Reserve · account.codex_weekly (account.resets_days:7)");
  });

  it("flattens every bucket window with a stable id-and-slot key", () => {
    const bars = codexRateLimitBars({
      rateLimitBuckets: [
        { limitId: "codex", primary: { usedPercent: 12, windowDurationMins: 300 } },
        {
          limitId: "base_model_inference",
          normalModelSlug: "gpt-reserve",
          primary: { usedPercent: 48, windowDurationMins: 10080 },
        },
      ],
    }, t);
    expect(bars.map((bar) => bar.key)).toEqual([
      "codex:primary",
      "base_model_inference:primary",
    ]);
    expect(bars[1].label).toContain("gpt-reserve");
  });

  it("falls back to the legacy primary and secondary fields", () => {
    const bars = codexRateLimitBars({
      primary: { usedPercent: 7, windowDurationMins: 10080 },
      secondary: { usedPercent: 2, windowDurationMins: 300 },
    }, t);
    expect(bars.map((bar) => bar.key)).toEqual(["codex:primary", "codex:secondary"]);
  });

  it("turns an old history point without bars into legacy series", () => {
    const bars = barsForPoint({
      ts: 1000,
      primaryPct: 7,
      secondaryPct: null,
      primaryWindowMins: 10080,
      secondaryWindowMins: null,
    });
    expect(bars).toEqual([{
      key: "codex:primary",
      windowKind: "primary",
      pct: 7,
      windowMins: 10080,
    }]);
  });
});
