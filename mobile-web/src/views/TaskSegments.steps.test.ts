import { describe, expect, it } from "vitest";
import type { SessionInfo, TaskPlanDetail } from "../types";
import { doneStepKeys, stepIdOf, subtaskStepLinks, subtasksForStep, type useSubtasks } from "./TaskSegments";

type Rows = ReturnType<typeof useSubtasks>;

describe("sub-tasks under steps", () => {
  const main = {
    id: "main",
    taskSummary: {
      subtaskSteps: [
        { taskId: "s1", planId: "demo", step: "P2" },
        { taskId: "s2", planId: "other", step: "P2" },
      ],
    },
  } as unknown as SessionInfo;
  const rows = ["s1", "s2", "s3"].map((line) => ({
    key: line,
    tip: { id: `${line}-tip`, taskId: line },
    hops: [],
    state: "running",
    pending: false,
  })) as unknown as Rows;

  it("reads the step id off a plan item", () => {
    expect(stepIdOf("**P2** — build")).toBe("P2");
    expect(stepIdOf("build")).toBeNull();
  });

  it("matches on both plan and step", () => {
    const links = subtaskStepLinks(main);
    const got = subtasksForStep(rows, links, "demo", "**P2** — build");
    expect(got.map((r) => r.tip.taskId)).toEqual(["s1"]);
    expect(subtasksForStep(rows, links, "demo", "**P1** — design")).toEqual([]);
    expect(subtasksForStep(rows, links, null, "**P2** — build")).toEqual([]);
  });
});

describe("doneStepKeys", () => {
  it("keys the checked-off steps by plan and step id", () => {
    const plans = [
      {
        id: "demo",
        items: [
          { text: "**P1** — design", done: true },
          { text: "**P2** — build", done: false },
          { text: "no step id", done: true },
        ],
      },
      { id: null, items: [{ text: "**P1** — loose", done: true }] },
    ] as unknown as TaskPlanDetail[];
    expect([...doneStepKeys(plans)]).toEqual(["demo\u0000P1"]);
  });
});
