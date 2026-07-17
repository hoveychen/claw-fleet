import { describe, expect, it } from "vitest";
import { shouldShowSkillInterop } from "./Onboarding";

describe("shouldShowSkillInterop", () => {
  it("shows the card only when Claude Code AND Codex are both present", () => {
    expect(shouldShowSkillInterop(true, { codex: true })).toBe(true);
  });

  it("hides the card when Codex is absent (single-runtime user)", () => {
    expect(shouldShowSkillInterop(true, { codex: false })).toBe(false);
  });

  it("hides the card when Claude Code is absent", () => {
    expect(shouldShowSkillInterop(false, { codex: true })).toBe(false);
  });

  it("hides the card when detected tools are unknown", () => {
    expect(shouldShowSkillInterop(true, undefined)).toBe(false);
    expect(shouldShowSkillInterop(true, null)).toBe(false);
  });

  it("treats a falsy hasClaudeCode (null/undefined status) as hidden", () => {
    // `hasClaudeCode` upstream is `status && (...)`, so it can be null/undefined.
    expect(shouldShowSkillInterop(null, { codex: true })).toBe(false);
    expect(shouldShowSkillInterop(undefined, { codex: true })).toBe(false);
  });
});
