import { describe, expect, it } from "vitest";
import { subtaskCountsAsFinished, subtaskOutcome } from "../../shared-ts/subtaskOutcome";
import en from "./locales/en.json";
import zh from "./locales/zh.json";

describe("subtaskOutcome", () => {
  it("reads reported back only on a clean end", () => {
    expect(subtaskOutcome("review", "clean")).toBe("reported");
    expect(subtaskOutcome("review", null)).toBe("reported");
    expect(subtaskOutcome("review", undefined)).toBe("reported");
    expect(subtaskOutcome("review", "midTurn")).toBe("midTurn");
    expect(subtaskOutcome("review", "interrupted")).toBe("interrupted");
    expect(subtaskOutcome("review", "authFailed")).toBe("authFailed");
    expect(subtaskOutcome("review", "failed")).toBe("failed");
  });

  it("has nothing to say about a sub-task that has not stopped", () => {
    expect(subtaskOutcome("running", "midTurn")).toBe(null);
    expect(subtaskOutcome("waiting", "clean")).toBe(null);
    expect(subtaskOutcome("done", "midTurn")).toBe(null);
  });

  it("has rail copy in both locales", () => {
    for (const k of ["subtask_mid_turn", "subtask_interrupted", "subtask_auth_failed", "subtask_failed"]) {
      expect((zh.home as Record<string, string>)[k]).toBeTruthy();
      expect((en.home as Record<string, string>)[k]).toBeTruthy();
    }
  });
});

describe("subtaskCountsAsFinished", () => {
  it("counts done and cleanly reported sub-tasks", () => {
    expect(subtaskCountsAsFinished("done", "midTurn")).toBe(true);
    expect(subtaskCountsAsFinished("review", "clean")).toBe(true);
  });

  it("keeps the old reading when the signal is missing", () => {
    expect(subtaskCountsAsFinished("review", null)).toBe(true);
    expect(subtaskCountsAsFinished("review", undefined)).toBe(true);
  });

  it("does not count a stopped sub-task that did not end clean", () => {
    for (const e of ["midTurn", "interrupted", "authFailed", "failed"] as const) {
      expect(subtaskCountsAsFinished("review", e)).toBe(false);
    }
  });

  it("does not count a sub-task that is still moving", () => {
    expect(subtaskCountsAsFinished("running", "clean")).toBe(false);
    expect(subtaskCountsAsFinished("waiting", null)).toBe(false);
    expect(subtaskCountsAsFinished("needsYou", null)).toBe(false);
  });
});
