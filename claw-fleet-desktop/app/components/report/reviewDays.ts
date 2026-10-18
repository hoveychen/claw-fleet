import type { ProjectMetrics, ReportSessionSummary } from "../../types";
import { repoRootPath } from "../../../../shared-ts/repoPath";

/** One project's day, as the review page shows it. */
export interface ProjectDay {
  path: string;
  name: string;
  costUsd: number;
  /** Tasks worked that day — top-level conversations only, costliest first. */
  tasks: ReportSessionSummary[];
  /** Subtasks (subagent / spawned sessions) the tasks ran. */
  subtaskCount: number;
}

function nameOf(path: string): string {
  const parts = path.replace(/\\/g, "/").split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/**
 * The daily report's per-workspace metrics regrouped by project: worktree
 * checkouts fold into their repo root, subagents become a count, and projects
 * are ordered by what they cost that day (the closest proxy the report has
 * for "where the work went").
 */
export function projectDays(projects: readonly ProjectMetrics[]): ProjectDay[] {
  const byPath = new Map<string, ProjectDay>();
  for (const p of projects) {
    const path = repoRootPath(p.workspacePath);
    let day = byPath.get(path);
    if (!day) {
      day = {
        path,
        name: path === p.workspacePath ? p.workspaceName || nameOf(path) : nameOf(path),
        costUsd: 0,
        tasks: [],
        subtaskCount: 0,
      };
      byPath.set(path, day);
    }
    day.costUsd += p.totalCostUsd ?? 0;
    for (const s of p.sessions) {
      if (s.isSubagent) day.subtaskCount += 1;
      else day.tasks.push(s);
    }
  }
  const days = [...byPath.values()].filter((d) => d.tasks.length > 0 || d.subtaskCount > 0);
  for (const d of days) d.tasks.sort((a, b) => b.costUsd - a.costUsd);
  return days.sort((a, b) => b.costUsd - a.costUsd || a.name.localeCompare(b.name));
}
