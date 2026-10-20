import { describe, expect, it } from "vitest";
import { subtaskOutcome } from "../../shared-ts/subtaskOutcome";
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
