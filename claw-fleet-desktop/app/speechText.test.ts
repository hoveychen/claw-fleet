import { describe, expect, it } from "vitest";

import { normalizeForSpeech } from "./decisionText";

/**
 * Every case here comes from a pronunciation the Edge TTS backend was measured
 * getting wrong, on strings mined out of `~/.fleet/decision-history`. The
 * mechanism is always the same: a polyphonic character ends up *isolated* — no
 * Chinese neighbours for segmentation — so the frontend falls back to the
 * character's most common reading. 重 falls back to zhòng (should be chóng).
 */
describe("normalizeForSpeech", () => {
  describe("markdown is removed, not turned into spaces", () => {
    // Old `lastQuestionSentence` did `.replace(/[`*_#>\[\]()]/g, " ")`.
    // Substituting space splits a word: `**重**试` -> `重 试`,
    // and isolated 重 is read zhòng. Deleting the marker keeps 重试 intact.
    it("keeps a word intact when a marker sits inside it", () => {
      expect(normalizeForSpeech("**重**试")).toBe("重试");
    });

    it("strips bold around a whole word", () => {
      expect(normalizeForSpeech("要不要**重试**一次？")).toBe("要不要重试一次？");
    });

    it("strips code spans, headings and list bullets", () => {
      expect(normalizeForSpeech("## 已修复")).toBe("已修复");
      expect(normalizeForSpeech("`重试`")).toBe("重试");
      expect(normalizeForSpeech("- 重试")).toBe("重试");
    });

    it("keeps ASCII words apart when a marker separated them", () => {
      // Deleting blindly would weld `foo` and `bar` into `foobar`.
      expect(normalizeForSpeech("**foo**bar")).toBe("foo bar");
    });
  });

  describe("an isolated 重 between ASCII is expanded to 重新", () => {
    // Measured: `10 finding 重 verify` -> zhòng (distance to 众 reference was
    // 0.0000 — frame-identical). Expanding to 重新 restores chóng on both
    // zh-CN and Multilingual voices.
    it("expands the real broadcast strings that were mispronounced", () => {
      expect(normalizeForSpeech("10 finding 重 verify")).toBe("10 finding 重新 verify");
      expect(normalizeForSpeech("Hub 重 build + force")).toBe("Hub 重新 build + force");
    });

    it("leaves 重 alone when it has Chinese neighbours", () => {
      // 严重 / 重新 / 重试 are already correctly segmented — touching them
      // would corrupt the text for no benefit.
      expect(normalizeForSpeech("这个 bug 很严重")).toBe("这个 bug 很严重");
      expect(normalizeForSpeech("正在重试")).toBe("正在重试");
      expect(normalizeForSpeech("卡在 docker 重新 build")).toBe("卡在 docker 重新 build");
    });
  });

  it("passes ordinary prose through untouched", () => {
    expect(normalizeForSpeech("改了 3 行代码，构建通过了")).toBe("改了 3 行代码，构建通过了");
  });
});
