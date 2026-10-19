import { describe, expect, it } from "vitest";
import type { PendingDecision, SessionInfo, SessionStatus } from "../types";
import { NEW_SESSION_ENTRYPOINT } from "../types";
import { speedTier } from "../../../shared-ts/taskSpeed";
import {
  buildHomeRows,
  filterRows,
  firstNeedsYou,
  nextNeedsYou,
  rowDecisions,
  rowDetail,
  rowOfSession,
  rowSessionId,
  mixedAgentSources,
  projectGroups,
  rowPreview,
  rowActiveSubtasks,
  rowRunningSubagents,
  rowTokenSpeed,
  sectionRows,
  stepText,
  type TaskRow,
} from "./homeRows";

function session(id: string, over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id,
    jsonlPath: `/sessions/${id}.jsonl`,
    workspacePath: "/work/fleet",
    workspaceName: "fleet",
    status: "idle" as SessionStatus,
    procAlive: false,
    entrypoint: NEW_SESSION_ENTRYPOINT,
    fleetSpawned: true,
    isSubagent: false,
    createdAtMs: 100,
    lastActivityMs: 1000,
    agentLastActivityMs: 1000,
    aiTitle: `title ${id}`,
    taskId: id,
    agentSource: "claude-code",
    ...over,
  } as SessionInfo;
}

function card(id: string, sessionId: string, arrivedAt = 5000): PendingDecision {
  return {
    kind: "fleet-ask",
    id,
    request: { id, sessionId, workspaceName: "elsewhere", aiTitle: "hand-opened", questions: [] },
    answers: {},
    arrivedAt,
  } as unknown as PendingDecision;
}

const opts = { isQuiet: () => false, untitled: "untitled" };

function build(sessions: SessionInfo[], decisions: PendingDecision[] = []) {
  return buildHomeRows(sessions, decisions, opts);
}

describe("buildHomeRows", () => {
  it("folds relay hops of one task into one row opened at the latest hop", () => {
    const rows = build([
      session("root", { handoff: { chainId: "c", chainLen: 2, hop: 1 } }),
      session("hop2", {
        taskId: "root",
        handoff: { chainId: "c", chainLen: 2, hop: 2 },
        lastActivityMs: 900,
      }),
    ]);
    expect(rows).toHaveLength(1);
    const row = rows[0] as TaskRow;
    expect(row.hops.map((h) => h.id).sort()).toEqual(["hop2", "root"]);
    expect(rowSessionId(row)).toBe("hop2");
  });

  it("files sub-tasks under their parent instead of listing them as tasks", () => {
    const rows = build([
      session("main"),
      session("sub", { parentTaskId: "main", taskId: "sub", status: "executing", procAlive: true }),
    ]);
    expect(rows).toHaveLength(1);
    const row = rows[0] as TaskRow;
    expect(row.subtasks.map((st) => st.tip.id)).toEqual(["sub"]);
    // The main line is idle but a sub-task works: the task waits.
    expect(row.state).toBe("waiting");
  });

  it("lists a sub-task as a task of its own when its parent has no row", () => {
    // Parent started in a terminal (not a Fleet task), or absent from the scan.
    const rows = build([
      session("cli", { entrypoint: "cli", fleetSpawned: false }),
      session("sub", { parentTaskId: "cli", taskId: "sub", aiTitle: "web" }),
      session("lost", { parentTaskId: "aged-out", taskId: "lost", status: "executing", procAlive: true }),
    ]);
    const tasks = rows.filter((r): r is TaskRow => r.kind === "task");
    expect(tasks.map((r) => r.tip.id).sort()).toEqual(["lost", "sub"]);
    expect(tasks.find((r) => r.tip.id === "lost")!.state).toBe("running");
    expect(rowOfSession(rows, "sub")?.key).toBe("sub");
  });

  it("puts a task whose sub-task holds a card in needs-you and says which one", () => {
    const rows = build(
      [session("main"), session("sub", { parentTaskId: "main", taskId: "sub", aiTitle: "web" })],
      [card("d1", "sub")],
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].state).toBe("needsYou");
    expect(rowDetail(rows[0], "untitled")).toEqual({ kind: "subtaskNeedsYou", subtaskTitle: "web" });
  });

  it("gives a card from a session outside every task its own needs-you row", () => {
    const handOpened = session("cli", { entrypoint: "cli", fleetSpawned: false });
    const rows = build([session("main"), handOpened], [card("d1", "cli"), card("d2", "gone")]);
    const cards = rows.filter((r) => r.kind === "card");
    expect(cards.map((r) => rowSessionId(r)).sort()).toEqual(["cli", "gone"]);
    expect(cards.every((r) => r.state === "needsYou")).toBe(true);
    const gone = cards.find((r) => rowSessionId(r) === "gone")!;
    expect(gone.title).toBe("hand-opened");
    expect(gone.projectName).toBe("elsewhere");
  });

  it("does not list sessions Fleet did not start", () => {
    expect(build([session("cli", { entrypoint: "cli", fleetSpawned: false })])).toEqual([]);
  });

  it("closes a task the boss finished even with nothing running", () => {
    const rows = build([session("main", { taskOutcome: "completed" })]);
    expect(rows[0].state).toBe("done");
    expect(rows[0].kind === "task" && rows[0].outcome).toBe("completed");
  });

  it("tells an abandoned task from a completed one", () => {
    const rows = build([session("main", { taskOutcome: "abandoned" })]);
    expect(rows[0].state).toBe("done");
    expect(rows[0].kind === "task" && rows[0].outcome).toBe("abandoned");
    expect(build([session("open")])[0]).toMatchObject({ outcome: null });
  });

  it("closes a stopped sub-task along with its accepted parent, not a moving one", () => {
    const rows = build([
      session("main", { taskOutcome: "completed" }),
      session("stopped", { parentTaskId: "main", taskId: "stopped" }),
      session("moving", { parentTaskId: "main", taskId: "moving", status: "executing", procAlive: true }),
    ]);
    const row = rows[0] as TaskRow;
    const byId = Object.fromEntries(row.subtasks.map((st) => [st.tip.id, st.state]));
    expect(byId).toEqual({ stopped: "done", moving: "running" });
  });

  it("ignores an error on a relay hop a later hop took over from", () => {
    // Seen on real data: hop 13 of a finished 14-hop chain ended serverErrored,
    // and the whole task sat in needs-you on the desktop but not the phone.
    const rows = build([
      session("root", {
        status: "serverErrored",
        taskOutcome: "completed",
        handoff: { chainId: "c", chainLen: 2, hop: 1 },
      }),
      session("hop2", {
        taskId: "root",
        taskOutcome: "completed",
        handoff: { chainId: "c", chainLen: 2, hop: 2 },
      }),
    ]);
    expect(rows[0].state).toBe("done");
  });
});

describe("sectionRows", () => {
  it("orders sections needs-you, running, waiting, review, done", () => {
    const rows = build(
      [
        session("done", { taskOutcome: "completed" }),
        session("review"),
        session("waiting", { status: "watching" }),
        session("running", { status: "executing", procAlive: true }),
        session("needs"),
      ],
      [card("d", "needs")],
    );
    expect(sectionRows(rows).map((s) => s.state)).toEqual([
      "needsYou",
      "running",
      "waiting",
      "review",
      "done",
    ]);
  });
});

describe("filterRows", () => {
  const rows = build([
    session("a", { workspacePath: "/work/fleet", aiTitle: "scan cpu" }),
    session("b", { workspacePath: "/work/studio", workspaceName: "studio", aiTitle: "shot 12" }),
    session("sub", { parentTaskId: "b", taskId: "sub", aiTitle: "storyboard" }),
  ]);
  const none = new Set<string>();

  it("scopes to one project", () => {
    const out = filterRows(rows, { scopedPath: "/work/studio", query: "", ftsMatchPaths: none });
    expect(out.map(rowSessionId)).toEqual(["b"]);
  });

  it("finds a task by the words of its sub-task", () => {
    const out = filterRows(rows, { scopedPath: null, query: "story", ftsMatchPaths: none });
    expect(out.map(rowSessionId)).toEqual(["b"]);
  });

  it("finds a task by a full-text hit on any of its sessions", () => {
    const out = filterRows(rows, {
      scopedPath: null,
      query: "zzz",
      ftsMatchPaths: new Set(["/sessions/sub.jsonl"]),
    });
    expect(out.map(rowSessionId)).toEqual(["b"]);
  });
});

describe("rowDetail", () => {
  it("reads the running line as step d/t, step text and sub-task tally", () => {
    const rows = build([
      session("main", {
        status: "executing",
        procAlive: true,
        taskPlan: { done: 2, total: 4, currentPlan: null, planId: "p", currentTask: null },
        taskSummary: {
          hopCount: 1,
          currentStep: "**P3** — run the `tests`",
          subtaskCount: 1,
          subtasksActive: 0,
          startedAtMs: 100,
          lastActivityMs: 1000,
          costUsd: 0,
        },
      }),
      session("sub", { parentTaskId: "main", taskId: "sub", taskOutcome: "completed" }),
    ]);
    expect(rowDetail(rows[0], "u")).toEqual({
      kind: "step",
      done: 2,
      total: 4,
      text: "run the tests",
      subtasks: { done: 1, total: 1 },
    });
  });

  it("reads the waiting line from the wait reason", () => {
    const rows = build([
      session("main", {
        status: "watching",
        lastActivityMs: 700,
        taskSummary: {
          hopCount: 1,
          waitReason: "CI",
          subtaskCount: 0,
          subtasksActive: 0,
          startedAtMs: 100,
          lastActivityMs: 700,
          costUsd: 0,
        },
      }),
    ]);
    expect(rowDetail(rows[0], "u")).toEqual({ kind: "wait", reason: "CI", sinceMs: 700 });
  });
});

describe("navigation helpers", () => {
  it("finds the row of a sub-task session and the first needs-you row", () => {
    const rows = build(
      [
        session("main", { lastActivityMs: 3000, agentLastActivityMs: 3000 }),
        session("sub", { parentTaskId: "main", taskId: "sub" }),
        session("other"),
      ],
      [card("d", "other")],
    );
    expect(rowSessionId(rowOfSession(rows, "sub")!)).toBe("main");
    expect(rowSessionId(firstNeedsYou(rows)!)).toBe("other");
  });

  it("strips the step id and markdown", () => {
    expect(stepText("**P12b** – do **it**")).toBe("do it");
    expect(stepText("")).toBeNull();
  });
});

describe("rowDecisions", () => {
  it("collects the cards of a task's sub-tasks and leaves out the open session's", () => {
    const rows = build(
      [session("main"), session("sub", { parentTaskId: "main", taskId: "sub" })],
      [card("c-main", "main"), card("c-sub", "sub"), card("c-other", "elsewhere")],
    );
    const row = rowOfSession(rows, "main")!;
    expect(rowDecisions(row, [card("c-main", "main"), card("c-sub", "sub"), card("c-other", "x")], "main").map((d) => d.id)).toEqual(["c-sub"]);
    expect(rowDecisions(row, [card("c-main", "main"), card("c-sub", "sub")]).map((d) => d.id)).toEqual(["c-main", "c-sub"]);
  });
});

describe("nextNeedsYou", () => {
  it("skips the row just answered and finds the next one waiting", () => {
    const rows = build(
      [session("a", { lastActivityMs: 3000 }), session("b", { lastActivityMs: 2000 }), session("c")],
      [card("ca", "a"), card("cb", "b")],
    );
    const a = rowOfSession(rows, "a")!;
    const b = rowOfSession(rows, "b")!;
    expect(nextNeedsYou(rows, a.key)?.key).toBe(b.key);
    expect(nextNeedsYou(rows.filter((r) => r.key !== b.key), a.key)).toBeNull();
  });
});

describe("projectGroups", () => {
  it("puts a project with something that needs you first and counts it", () => {
    const rows = build(
      [
        session("a-run", {
          workspacePath: "/work/a",
          workspaceName: "a",
          status: "executing",
          procAlive: true,
          lastActivityMs: 9000,
        }),
        session("b-done", {
          workspacePath: "/work/b",
          workspaceName: "b",
          taskOutcome: "completed",
          lastActivityMs: 3000,
        }),
        session("b-needs", { workspacePath: "/work/b", workspaceName: "b", lastActivityMs: 2000 }),
        session("b-review", { workspacePath: "/work/b", workspaceName: "b", lastActivityMs: 4000 }),
      ],
      [card("d", "b-needs", 1)],
    );
    const groups = projectGroups(rows);
    expect(groups.map((g) => g.name)).toEqual(["b", "a"]);
    const b = groups[0];
    expect(b.needsYou).toBe(1);
    // Open rows by state ("needs you" above the newer review); done set apart.
    expect(b.rows.map((r) => r.key)).toEqual(["b-needs", "b-review"]);
    expect(b.doneRows.map((r) => r.key)).toEqual(["b-done"]);
  });

  it("keeps recency order between projects that need nothing", () => {
    const rows = build([
      session("old", { workspacePath: "/work/old", workspaceName: "old", agentLastActivityMs: 1000 }),
      session("new", { workspacePath: "/work/new", workspaceName: "new", agentLastActivityMs: 5000 }),
    ]);
    expect(projectGroups(rows).map((g) => g.name)).toEqual(["new", "old"]);
  });
});

describe("row signals", () => {
  it("shows the source glyph only when agents are mixed", () => {
    const same = build([session("a"), session("b")]);
    expect(mixedAgentSources(same)).toBe(false);
    const mixed = build([session("a"), session("b", { agentSource: "codex" })]);
    expect(mixedAgentSources(mixed)).toBe(true);
  });

  it("adds up running subagents across the main line and its sub-tasks", () => {
    const rows = build([
      session("root", { runningSubagentCount: 2 }),
      session("sub", { taskId: "sub", parentTaskId: "root", runningSubagentCount: 1 }),
    ]);
    expect(rowRunningSubagents(rows[0])).toBe(3);
  });

  it("sums the task's speed over the open main hop and each sub-task's open hop", () => {
    const rows = build([
      session("root", { handoff: { chainId: "c", chainLen: 2, hop: 1 }, agentTokenSpeed: 90 }),
      session("hop2", {
        taskId: "root",
        handoff: { chainId: "c", chainLen: 2, hop: 2 },
        agentTokenSpeed: 30,
      }),
      session("sub", { taskId: "sub", parentTaskId: "root", agentTokenSpeed: 25 }),
    ]);
    // The retired first hop's leftover speed is not counted.
    expect(rowTokenSpeed(rows[0])).toBe(55);
  });

  it("maps speed to tiers: still below 1 tok/s, arc only with helpers fanned out", () => {
    expect(speedTier(0)).toBe(0);
    expect(speedTier(0.6)).toBe(0);
    expect(speedTier(undefined)).toBe(0);
    expect(speedTier(Number.NaN)).toBe(0);
    expect(speedTier(1)).toBe(1);
    expect(speedTier(39.9)).toBe(1);
    expect(speedTier(40)).toBe(2);
    expect(speedTier(149)).toBe(2);
    expect(speedTier(150)).toBe(3);
  });

  it("counts the sub-tasks still at it, not the ones that reported back", () => {
    const rows = build([
      session("root"),
      session("live", { taskId: "live", parentTaskId: "root", status: "executing", procAlive: true }),
      session("over", { taskId: "over", parentTaskId: "root" }),
    ]);
    expect(rowActiveSubtasks(rows[0])).toBe(1);
  });

  it("previews the last message unless it only repeats the title", () => {
    const [row] = build([session("a", { lastMessagePreview: "  all  tests\npass " })]);
    expect(rowPreview(row)).toBe("all tests pass");
    const [same] = build([session("b", { aiTitle: "fix it", lastMessagePreview: "fix it" })]);
    expect(rowPreview(same)).toBeNull();
  });
});
