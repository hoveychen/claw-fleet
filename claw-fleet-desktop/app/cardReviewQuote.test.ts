// The quote a card review ask carries. The host's answer digest names the
// option a steelman was about by finding exactly one option label in the quote
// (`session_explain::digest_subject`), so the shape is a contract, and the
// option tag must survive a question long enough to hit the quote cap.
import { describe, expect, it } from "vitest";
import {
  MAX_QUOTE_CHARS,
  STEELMAN_OPTION_TAG,
  cardReviewRunning,
  cardReviewSubject,
  flipConditionsQuote,
  steelmanQuote,
} from "./selectionExplain";

describe("card review quotes", () => {
  it("steelman: question, blank line, tagged option with its description", () => {
    expect(steelmanQuote("  先灰度还是直接全量？ ", { label: "直接全量", description: "跳过灰度" })).toBe(
      "先灰度还是直接全量？\n\n【被追问的选项】直接全量 — 跳过灰度",
    );
  });

  it("steelman: an option without a description is the label alone", () => {
    expect(steelmanQuote("Q", { label: "照做", description: "" })).toBe("Q\n\n【被追问的选项】照做");
  });

  it("cuts the question, never the option tag, at the quote cap", () => {
    const q = steelmanQuote("长".repeat(MAX_QUOTE_CHARS * 2), { label: "方案B", description: "说明" });
    expect(q.length).toBeLessThanOrEqual(MAX_QUOTE_CHARS);
    expect(q.endsWith(`${STEELMAN_OPTION_TAG}方案B — 说明`)).toBe(true);
  });

  it("flip conditions: the question itself, capped", () => {
    expect(flipConditionsQuote(" 要不要上线？\n")).toBe("要不要上线？");
    expect(flipConditionsQuote("x".repeat(MAX_QUOTE_CHARS + 5)).length).toBe(MAX_QUOTE_CHARS);
  });

  it("the answer heading reads the option back out of a steelman quote", () => {
    const quote = steelmanQuote("先灰度还是直接全量？", { label: "直接全量", description: "跳过灰度" });
    expect(cardReviewSubject({ preset: "steelman", quote })).toEqual({ preset: "steelman", option: "直接全量" });
    expect(cardReviewSubject({ preset: "steelman", quote: "Q\n\n【被追问的选项】照做" })).toEqual({
      preset: "steelman",
      option: "照做",
    });
    expect(cardReviewSubject({ preset: "flip_conditions", quote: "Q" })).toEqual({ preset: "flip_conditions" });
    // A steelman asked from a plain selection names no option: headed by its quote.
    expect(cardReviewSubject({ preset: "steelman", quote: "灰度到 5%" })).toBeNull();
    expect(cardReviewSubject({ preset: "explain", quote: "Q" })).toBeNull();
  });

  it("a review button waits only on its own running ask", () => {
    const recs = [{ preset: "steelman", quote: "a", status: "running" }, { preset: "steelman", quote: "b", status: "done" }];
    expect(cardReviewRunning(recs, "steelman", "a")).toBe(true);
    expect(cardReviewRunning(recs, "steelman", "b")).toBe(false);
    expect(cardReviewRunning(recs, "flip_conditions", "a")).toBe(false);
  });
});
