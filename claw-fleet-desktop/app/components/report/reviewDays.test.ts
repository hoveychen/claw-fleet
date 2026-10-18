import { describe, expect, it } from "vitest";
import type { ProjectMetrics, ReportSessionSummary } from "../../types";
import { projectDays } from "./reviewDays";

const session = (id: string, costUsd: number, isSubagent = false): ReportSessionSummary => ({
  id,
  title: id,
  lastMessage: null,
  model: null,
  isSubagent,
  outputTokens: 0,
  costUsd,
  agentSource: "claude",
});

const metrics = (workspacePath: string, workspaceName: string, cost: number, sessions: ReportSessionSummary[]) =>
  ({ workspacePath, workspaceName, totalCostUsd: cost, sessions }) as unknown as ProjectMetrics;

describe("projectDays", () => {
  it("folds worktrees into their project and counts subtasks apart from tasks", () => {
    const days = projectDays([
      metrics("/u/fleet", "fleet", 1, [session("a", 1)]),
      metrics("/u/fleet/.worktrees/p1", "p1", 3, [session("b", 2), session("c", 1, true)]),
      metrics("/u/site", "site", 2, [session("d", 2)]),
    ]);
    expect(days.map((d) => [d.name, d.costUsd, d.tasks.map((t) => t.id), d.subtaskCount])).toEqual([
      ["fleet", 4, ["b", "a"], 1],
      ["site", 2, ["d"], 0],
    ]);
  });

  it("drops projects with no sessions", () => {
    expect(projectDays([metrics("/u/x", "x", 0, [])])).toEqual([]);
  });
});
