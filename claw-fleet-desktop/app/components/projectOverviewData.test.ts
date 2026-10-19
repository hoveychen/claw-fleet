import { describe, expect, it } from "vitest";
import type { HomeRow } from "./homeRows";
import type { ProcRecord } from "../types";
import {
  OVERVIEW_RECENT_LIMIT,
  overviewLibrary,
  overviewRunningProcs,
  overviewTasks,
} from "./projectOverviewData";

const P = "/w/app";

function row(key: string, state: HomeRow["state"], projectPath: string | null = P): HomeRow {
  return { kind: "card", key, state, projectPath, title: key } as unknown as HomeRow;
}

function proc(id: string, workspacePath: string, status: ProcRecord["status"], startedMs: number): ProcRecord {
  return { id, workspacePath, status, startedMs, command: id, cols: 80, rows: 24 } as ProcRecord;
}

describe("overviewTasks", () => {
  it("keeps only the project's rows and orders open ones by state, recency inside a state", () => {
    const rows = [
      row("run-new", "running"),
      row("other", "needsYou", "/w/other"),
      row("review", "review"),
      row("run-old", "running"),
      row("needs", "needsYou"),
    ];
    const got = overviewTasks(rows, P);
    expect(got.open.map((r) => r.key)).toEqual(["needs", "run-new", "run-old", "review"]);
    expect(got.doneTotal).toBe(0);
  });

  it("caps the recently done list but counts every closed task", () => {
    const rows = Array.from({ length: OVERVIEW_RECENT_LIMIT + 3 }, (_, i) => row(`d${i}`, "done"));
    const got = overviewTasks(rows, P);
    expect(got.recentDone.map((r) => r.key)).toEqual(rows.slice(0, OVERVIEW_RECENT_LIMIT).map((r) => r.key));
    expect(got.doneTotal).toBe(OVERVIEW_RECENT_LIMIT + 3);
    expect(got.open).toEqual([]);
  });
});

describe("overviewRunningProcs", () => {
  it("counts live commands in the project's worktrees too, newest first", () => {
    const procs = [
      proc("old", P, "running", 1),
      proc("exited", P, "exited", 5),
      proc("wt", `${P}/.worktrees/feat`, "starting", 3),
      proc("elsewhere", "/w/other", "running", 9),
    ];
    expect(overviewRunningProcs(procs, P).map((p) => p.id)).toEqual(["wt", "old"]);
  });
});

describe("overviewLibrary", () => {
  it("files a worktree's item under its project", () => {
    const items = [
      { type: "doc", key: "doc:a", workspacePath: `${P}/.worktrees/x` },
      { type: "doc", key: "doc:b", workspacePath: "/w/other" },
      { type: "artifact", key: "artifact:c", workspacePath: P },
    ] as never[];
    expect(overviewLibrary(items, P).map((it: { key: string }) => it.key)).toEqual(["doc:a", "artifact:c"]);
  });
});
