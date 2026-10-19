import type { HomeRow } from "./homeRows";
import { HOME_SECTION_ORDER } from "./homeRows";
import type { ProcRecord } from "../types";
import { libraryProject, type LibraryItem } from "../../../shared-ts/library";
import { repoRootPath } from "../../../shared-ts/repoPath";

/** How many closed tasks / library items the overview lists before deferring
 *  to the page that owns them. */
export const OVERVIEW_RECENT_LIMIT = 5;

export interface OverviewTasks {
  /** Open tasks, "needs you" first, then in `HOME_SECTION_ORDER`. */
  open: HomeRow[];
  /** The most recently closed tasks, newest first. */
  recentDone: HomeRow[];
  /** Every closed task of the project, for the "see all" count. */
  doneTotal: number;
}

/** The project's slice of the task list. `rows` arrive newest first, the way
 *  `buildHomeRows` returns them. */
export function overviewTasks(rows: readonly HomeRow[], projectPath: string): OverviewTasks {
  const mine = rows.filter((r) => r.projectPath === projectPath);
  const rank = (r: HomeRow) => HOME_SECTION_ORDER.indexOf(r.state);
  // Array.prototype.sort is stable, so recency holds inside a state.
  const open = mine.filter((r) => r.state !== "done").sort((a, b) => rank(a) - rank(b));
  const done = mine.filter((r) => r.state === "done");
  return { open, recentDone: done.slice(0, OVERVIEW_RECENT_LIMIT), doneTotal: done.length };
}

/** Commands still running anywhere in the project, worktrees included. */
export function overviewRunningProcs(procs: readonly ProcRecord[], projectPath: string): ProcRecord[] {
  return procs
    .filter((p) => p.status !== "exited" && repoRootPath(p.workspacePath) === projectPath)
    .sort((a, b) => b.startedMs - a.startedMs);
}

/** The project's library items, newest first (the merged list's own order). */
export function overviewLibrary<I extends LibraryItem>(items: readonly I[], projectPath: string): I[] {
  return items.filter((it) => libraryProject(it) === projectPath);
}
