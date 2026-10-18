// Five-state task derivation (shared-ts/taskState.ts), shared by the desktop
// home and the phone.
import { describe, expect, it } from "vitest";
import {
  TASK_STATES,
  taskStateOf,
  taskStateRank,
  type TaskMemberView,
} from "../../shared-ts/taskState";

function m(status: string, extra: Partial<TaskMemberView> = {}): TaskMemberView {
  return { status, procAlive: false, quiet: false, hasPendingDecision: false, ...extra };
}

describe("taskStateOf", () => {
  it("a pending card anywhere in the task wins, even over a closed task", () => {
    expect(
      taskStateOf({ main: [m("idle")], subtasks: [m("waitingInput", { hasPendingDecision: true })], outcome: "completed" }),
    ).toBe("needsYou");
  });

  it("an errored sub-task needs the boss", () => {
    expect(taskStateOf({ main: [m("executing")], subtasks: [m("stuck")], outcome: null })).toBe("needsYou");
  });

  it("a closed task is done", () => {
    expect(taskStateOf({ main: [m("idle")], subtasks: [], outcome: "abandoned" })).toBe("done");
  });

  it("a main hop in flight is running", () => {
    expect(taskStateOf({ main: [m("idle"), m("thinking")], subtasks: [], outcome: null })).toBe("running");
    expect(taskStateOf({ main: [m("idle", { quiet: true })], subtasks: [], outcome: null })).toBe("running");
  });

  it("a main hop parked on a watch, or working sub-tasks, is waiting", () => {
    expect(taskStateOf({ main: [m("watching")], subtasks: [], outcome: null })).toBe("waiting");
    expect(taskStateOf({ main: [m("idle")], subtasks: [m("executing")], outcome: null })).toBe("waiting");
  });

  it("a parked waitingInput process without a card is not running", () => {
    expect(
      taskStateOf({ main: [m("waitingInput", { procAlive: true })], subtasks: [], outcome: null }),
    ).toBe("review");
  });

  it("a stopped, unclosed task waits for review", () => {
    expect(taskStateOf({ main: [m("idle")], subtasks: [m("idle")], outcome: null })).toBe("review");
  });

  it("ranks follow the section order", () => {
    expect(TASK_STATES.map(taskStateRank)).toEqual([0, 1, 2, 3, 4]);
  });
});
