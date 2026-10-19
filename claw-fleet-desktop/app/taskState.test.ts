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

  it("an error on a superseded hop is history, a card on one is not", () => {
    const old = m("serverErrored", { superseded: true });
    expect(taskStateOf({ main: [old, m("executing")], subtasks: [], outcome: null })).toBe("running");
    expect(
      taskStateOf({ main: [{ ...old, hasPendingDecision: true }, m("idle")], subtasks: [], outcome: null }),
    ).toBe("needsYou");
  });

  it("a closed task is done", () => {
    expect(taskStateOf({ main: [m("idle")], subtasks: [], outcome: "abandoned" })).toBe("done");
  });

  it("closing a task answers its error, but not its card", () => {
    expect(taskStateOf({ main: [m("rateLimited")], subtasks: [], outcome: "completed" })).toBe("done");
    expect(
      taskStateOf({ main: [m("idle", { hasPendingDecision: true })], subtasks: [], outcome: "completed" }),
    ).toBe("needsYou");
  });

  it("a main hop in flight is running", () => {
    expect(taskStateOf({ main: [m("idle"), m("thinking")], subtasks: [], outcome: null })).toBe("running");
    expect(taskStateOf({ main: [m("idle", { quiet: true })], subtasks: [], outcome: null })).toBe("running");
  });

  it("a main hop or sub-task parked on a watch, with nothing in flight, is waiting", () => {
    expect(taskStateOf({ main: [m("watching")], subtasks: [], outcome: null })).toBe("waiting");
    expect(taskStateOf({ main: [m("idle")], subtasks: [m("watching")], outcome: null })).toBe("waiting");
  });

  it("a sub-task in flight makes the task running even while the main hop watches", () => {
    expect(taskStateOf({ main: [m("idle")], subtasks: [m("executing")], outcome: null })).toBe("running");
    expect(
      taskStateOf({ main: [m("watching")], subtasks: [m("executing"), m("thinking")], outcome: null }),
    ).toBe("running");
  });

  it("a parked waitingInput process without a card is not running", () => {
    expect(
      taskStateOf({ main: [m("waitingInput", { procAlive: true })], subtasks: [], outcome: null }),
    ).toBe("review");
  });

  it("a main hop that ended its turn to wait on background subagents is waiting", () => {
    const parked = m("waitingInput", { procAlive: true, runningSubagents: 3 });
    expect(taskStateOf({ main: [parked], subtasks: [], outcome: null })).toBe("waiting");
    expect(
      taskStateOf({ main: [m("idle")], subtasks: [m("waitingInput", { runningSubagents: 1 })], outcome: null }),
    ).toBe("waiting");
  });

  it("subagents on a superseded hop do not hold the task out of review", () => {
    const old = m("idle", { superseded: true, runningSubagents: 2 });
    expect(taskStateOf({ main: [old, m("idle")], subtasks: [], outcome: null })).toBe("review");
  });

  it("a stopped, unclosed task waits for review", () => {
    expect(taskStateOf({ main: [m("idle")], subtasks: [m("idle")], outcome: null })).toBe("review");
  });

  it("ranks follow the section order", () => {
    expect(TASK_STATES.map(taskStateRank)).toEqual([0, 1, 2, 3, 4]);
  });
});
