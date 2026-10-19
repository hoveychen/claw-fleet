import { describe, expect, it } from "vitest";
import {
  buildTaskRows,
  projectSectionRows,
  rowDetail,
  runningSubagents,
  sectionRows,
  stepText,
  subtaskRowsOf,
  subtaskTally,
  taskSessionIds,
} from "./taskRows";
import type { SessionInfo } from "./types";

type Row = SessionInfo & { deviceId: string };

let clock = 1_000;
function s(over: Partial<Row>): Row {
  clock += 1;
  return {
    id: over.id ?? `s${clock}`,
    deviceId: "d",
    workspacePath: "/work/fleet",
    workspaceName: "fleet",
    status: "idle",
    isSubagent: false,
    entrypoint: "claw-fleet-newsession",
    lastActivityMs: clock,
    createdAtMs: clock,
    jsonlPath: `/p/${over.id ?? clock}.jsonl`,
    ...over,
  } as Row;
}

function build(sessions: Row[], pending: string[] = []) {
  return buildTaskRows(sessions, {
    isPending: (_d, id) => pending.includes(id),
    isQuiet: () => false,
    multiDevice: false,
    untitled: "untitled",
  });
}

describe("buildTaskRows", () => {
  it("folds relay hops into one task opened at the latest hop", () => {
    const rows = build([
      s({ id: "h1", taskId: "h1", handoff: { chainId: "c", hop: 1, chainLen: 2 } as never }),
      s({ id: "h2", taskId: "h1", handoff: { chainId: "c", hop: 2, chainLen: 2 } as never }),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].tip.id).toBe("h2");
    expect(rows[0].hops.map((h) => h.id).sort()).toEqual(["h1", "h2"]);
  });

  it("lists a sub-task as a task of its own when its parent has no row", () => {
    const rows = build([
      s({ id: "cli", taskId: "cli", entrypoint: "cli" }),
      s({ id: "sub", taskId: "sub", parentTaskId: "cli" }),
      s({ id: "lost", taskId: "lost", parentTaskId: "aged-out", status: "executing", procAlive: true }),
    ]);
    expect(rows.map((r) => r.tip.id).sort()).toEqual(["lost", "sub"]);
    expect(rows.find((r) => r.tip.id === "lost")!.state).toBe("running");
  });

  it("puts sub-tasks under their task instead of on the list", () => {
    const rows = build([
      s({ id: "t", taskId: "t" }),
      s({ id: "sub", taskId: "sub", parentTaskId: "t", status: "executing", procAlive: true }),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].subtasks.map((st) => st.tip.id)).toEqual(["sub"]);
    // The main hop is idle but a sub-task works: the task is waiting, not done.
    expect(rows[0].state).toBe("waiting");
  });

  it("files a task under needs-you when a sub-task has a card waiting", () => {
    const rows = build(
      [s({ id: "t", taskId: "t" }), s({ id: "sub", taskId: "sub", parentTaskId: "t", aiTitle: "web" })],
      ["sub"],
    );
    expect(rows[0].state).toBe("needsYou");
    expect(rowDetail(rows[0], "untitled")).toEqual({ kind: "subtaskNeedsYou", subtaskTitle: "web" });
  });

  it("leaves out sessions Fleet did not start, and subagents", () => {
    const rows = build([
      s({ id: "cli", entrypoint: "cli" }),
      s({ id: "agent-x", isSubagent: true }),
    ]);
    expect(rows).toHaveLength(0);
  });

  it("does not join a sub-task to a same-id task on another device", () => {
    const rows = build([
      s({ id: "t", taskId: "t", deviceId: "a" }),
      s({ id: "sub", taskId: "sub", parentTaskId: "t", deviceId: "b" }),
    ]);
    expect(rows[0].subtasks).toHaveLength(0);
  });

  it("ignores an error on a hop a later hop already took over", () => {
    const rows = build([
      s({ id: "h13", taskId: "h1", status: "serverErrored", handoff: { chainId: "c", hop: 13, chainLen: 14 } as never }),
      s({ id: "h14", taskId: "h1", status: "executing", procAlive: true, handoff: { chainId: "c", hop: 14, chainLen: 14 } as never }),
    ]);
    expect(rows[0].state).toBe("running");
  });

  it("still flags an error on the latest hop", () => {
    const rows = build([
      s({ id: "h1", taskId: "h1", handoff: { chainId: "c", hop: 1, chainLen: 2 } as never }),
      s({ id: "h2", taskId: "h1", status: "stuck", handoff: { chainId: "c", hop: 2, chainLen: 2 } as never }),
    ]);
    expect(rows[0].state).toBe("needsYou");
    expect(rowDetail(rows[0], "untitled")).toMatchObject({ kind: "error", status: "stuck" });
  });

  it("reads a closed task as done", () => {
    const rows = build([s({ id: "t", taskId: "t", taskOutcome: "completed" })]);
    expect(rows[0].state).toBe("done");
    expect(rows[0].outcome).toBe("completed");
    expect(build([s({ id: "a", taskId: "a", taskOutcome: "abandoned" })])[0].outcome).toBe("abandoned");
  });

  it("closes a stopped sub-task along with its accepted parent, not a moving one", () => {
    const rows = build([
      s({ id: "t", taskId: "t", taskOutcome: "completed" }),
      s({ id: "stopped", taskId: "stopped", parentTaskId: "t" }),
      s({ id: "moving", taskId: "moving", parentTaskId: "t", status: "executing", procAlive: true }),
    ]);
    const byId = Object.fromEntries(rows[0].subtasks.map((st) => [st.tip.id, st.state]));
    expect(byId).toEqual({ stopped: "done", moving: "running" });
  });
});

describe("sectionRows", () => {
  it("orders sections the way the boss reads them and drops empty ones", () => {
    const rows = build(
      [
        s({ id: "run", taskId: "run", status: "executing", procAlive: true }),
        s({ id: "rev", taskId: "rev" }),
        s({ id: "ask", taskId: "ask" }),
      ],
      ["ask"],
    );
    expect(sectionRows(rows).map((sec) => sec.state)).toEqual(["needsYou", "review", "running"]);
  });
});

describe("projectSectionRows", () => {
  it("pins the chat folder, orders projects by name and puts needs-you rows first", () => {
    const rows = build(
      [
        s({ id: "z1", taskId: "z1", workspacePath: "/work/zeta", workspaceName: "zeta" }),
        s({ id: "a1", taskId: "a1", workspacePath: "/work/alpha", workspaceName: "alpha", status: "executing", procAlive: true }),
        s({ id: "a2", taskId: "a2", workspacePath: "/work/alpha", workspaceName: "alpha" }),
        s({ id: "c1", taskId: "c1", workspacePath: "/home/.fleet/chat", workspaceName: "chat" }),
      ],
      ["a2"],
    );
    const secs = projectSectionRows(rows, (k) => k === "/home/.fleet/chat");
    expect(secs.map((sec) => [sec.name, sec.chat])).toEqual([
      ["chat", true],
      ["alpha", false],
      ["zeta", false],
    ]);
    const alpha = secs[1];
    expect(alpha.needsYou).toBe(1);
    expect(alpha.rows.map((r) => r.tip.id)).toEqual(["a2", "a1"]);
  });
});

describe("runningSubagents", () => {
  it("sums live subagents over every main hop", () => {
    const rows = build([
      s({ id: "h1", taskId: "h1", runningSubagentCount: 1, handoff: { chainId: "c", hop: 1, chainLen: 2 } as never }),
      s({ id: "h2", taskId: "h1", runningSubagentCount: 2, handoff: { chainId: "c", hop: 2, chainLen: 2 } as never }),
    ]);
    expect(runningSubagents(rows[0])).toBe(3);
  });
});

describe("rowDetail", () => {
  it("names the step a running task is on", () => {
    const rows = build([
      s({
        id: "t",
        taskId: "t",
        status: "executing",
        procAlive: true,
        taskPlan: { done: 2, total: 4, currentPlan: null, planId: "p", currentTask: "**P3** — run tests" },
      }),
    ]);
    expect(rowDetail(rows[0], "untitled")).toEqual({
      kind: "step",
      done: 2,
      total: 4,
      text: "run tests",
    });
  });

  it("says what a waiting task waits for", () => {
    const rows = build([
      s({
        id: "t",
        taskId: "t",
        status: "watching",
        taskSummary: { waitReason: "CI 通过" } as never,
      }),
    ]);
    expect(rowDetail(rows[0], "untitled")).toEqual({ kind: "wait", reason: "CI 通过" });
  });
});

describe("stepText", () => {
  it("drops the step id and markdown", () => {
    expect(stepText("**P2** — x")).toBe("x");
    expect(stepText("P10 - do it")).toBe("do it");
    expect(stepText(null)).toBe(null);
  });
});

describe("subtaskTally", () => {
  it("counts finished sub-tasks out of all", () => {
    const rows = build([
      s({ id: "t", taskId: "t" }),
      s({ id: "a", taskId: "a", parentTaskId: "t" }),
      s({ id: "b", taskId: "b", parentTaskId: "t", status: "executing", procAlive: true }),
    ]);
    expect(subtaskTally(rows[0])).toEqual({ done: 1, total: 2 });
  });
});

describe("subtaskRowsOf", () => {
  const opts = (pending: string[] = []) => ({
    isPending: (x: Row) => pending.includes(x.id),
    isQuiet: () => false,
  });

  it("lists only the given task's sub-tasks, relay hops folded", () => {
    const rows = subtaskRowsOf(
      [
        s({ id: "t", taskId: "t" }),
        s({ id: "a1", taskId: "a1", parentTaskId: "t", handoff: { chainId: "ca", hop: 1, chainLen: 2 } as never }),
        s({ id: "a2", taskId: "a1", parentTaskId: "t", handoff: { chainId: "ca", hop: 2, chainLen: 2 } as never }),
        s({ id: "other", taskId: "other", parentTaskId: "x" }),
        s({ id: "agent", parentTaskId: "t", isSubagent: true }),
      ],
      "t",
      opts(),
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].tip.id).toBe("a2");
    expect(rows[0].hops).toHaveLength(2);
  });

  it("files a sub-task with a waiting card under needsYou", () => {
    const rows = subtaskRowsOf(
      [s({ id: "b", taskId: "b", parentTaskId: "t", procAlive: true, status: "waitingInput" })],
      "t",
      opts(["b"]),
    );
    expect(rows[0].pending).toBe(true);
    expect(rows[0].state).toBe("needsYou");
  });

  it("files a stopped sub-task of an accepted task under done", () => {
    const sessions = [
      s({ id: "t", taskId: "t", taskOutcome: "completed" }),
      s({ id: "b", taskId: "b", parentTaskId: "t" }),
    ];
    expect(subtaskRowsOf(sessions, "t", opts())[0].state).toBe("done");
    expect(subtaskRowsOf(sessions.slice(1), "t", opts())[0].state).toBe("review");
  });
});

describe("taskSessionIds", () => {
  it("collects main hops and sub-task hops of the task", () => {
    const all = [
      s({ id: "t", taskId: "t" }),
      s({ id: "t2", taskId: "t" }),
      s({ id: "sub", taskId: "sub", parentTaskId: "t" }),
      s({ id: "x", taskId: "x" }),
    ];
    expect([...taskSessionIds(all, all[1])].sort()).toEqual(["sub", "t", "t2"]);
  });

  it("is just the session itself without a task id", () => {
    const one = s({ id: "lone" });
    expect([...taskSessionIds([one], one)]).toEqual(["lone"]);
  });
});

describe("buildTaskRows across a wake-up", () => {
  it("an answered card keeps the task out of review until the transcript moves", async () => {
    const { createWakeLatch, isParkedForWake, stickyWaking } = await import("../../shared-ts/wakeLatch");
    const latch = createWakeLatch();
    const rows = (sessions: Row[], pending: string[]) =>
      buildTaskRows(sessions, {
        isPending: (_d, id) => pending.includes(id),
        isQuiet: () => false,
        isWaking: (x, p) =>
          stickyWaking(latch, x.id, {
            parked: isParkedForWake(x, p),
            alive: !!x.procAlive,
            lastActivityMs: x.lastActivityMs,
            now: 0,
          }),
        multiDevice: false,
        untitled: "untitled",
      })[0].state;
    const parked = s({ id: "t", taskId: "t", fleetSpawned: true, status: "waitingInput", lastActivityMs: 10 });
    expect(rows([parked], ["t"])).toBe("needsYou");
    expect(rows([parked], [])).toBe("running");
    expect(rows([{ ...parked, lastActivityMs: 20 }], [])).toBe("review");
  });
});
