import { describe, expect, it } from "vitest";
import { subtaskCountsAsFinished, subtaskOutcome, subtaskOutcomeOnStep } from "../../shared-ts/subtaskOutcome";
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
    for (const k of ["subtask_mid_turn", "subtask_interrupted", "subtask_auth_failed", "subtask_failed", "subtask_taken_over"]) {
      expect((zh.home as Record<string, string>)[k]).toBeTruthy();
      expect((en.home as Record<string, string>)[k]).toBeTruthy();
    }
  });
});

describe("subtaskOutcomeOnStep", () => {
  const failed = { state: "review" as const, lastTurnEnd: "authFailed" as const, createdAtMs: 100 };

  it("keeps a failure red while nothing has moved past it", () => {
    expect(subtaskOutcomeOnStep(failed, { done: false, siblingCreatedAtMs: [100] })).toBe("authFailed");
    expect(subtaskOutcomeOnStep(failed, { done: false, siblingCreatedAtMs: [50, 100] })).toBe("authFailed");
  });

  it("reads taken over once the step is checked off", () => {
    expect(subtaskOutcomeOnStep(failed, { done: true, siblingCreatedAtMs: [100] })).toBe("takenOver");
  });

  it("reads taken over once a later sub-task works the same step", () => {
    expect(subtaskOutcomeOnStep(failed, { done: false, siblingCreatedAtMs: [100, 200] })).toBe("takenOver");
    const midTurn = { ...failed, lastTurnEnd: "midTurn" as const };
    expect(subtaskOutcomeOnStep(midTurn, { done: false, siblingCreatedAtMs: [200] })).toBe("takenOver");
  });

  it("leaves clean, missing-signal and still-moving sub-tasks alone", () => {
    const step = { done: true, siblingCreatedAtMs: [200] };
    expect(subtaskOutcomeOnStep({ ...failed, lastTurnEnd: "clean" }, step)).toBe("reported");
    expect(subtaskOutcomeOnStep({ ...failed, lastTurnEnd: null }, step)).toBe("reported");
    expect(subtaskOutcomeOnStep({ ...failed, state: "running" }, step)).toBe(null);
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
