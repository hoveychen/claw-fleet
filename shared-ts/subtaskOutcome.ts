/** How a stopped sub-task's row reads, shared by the desktop rail and the
 *  phone's sub-task lists.
 *
 *  A sub-task in the "review" state has stopped, but stopping is not the same
 *  as reporting back: a process killed mid-turn, a turn cut off by hand, or an
 *  auth failure stop the same way. `lastTurnEnd` is the core's reading of the
 *  transcript tail (`claw_fleet_core::session::TurnEnd`, the same split the
 *  sub-task recovery block uses), so only a clean end says "reported back". */

import type { TaskState } from "./taskState";

/** Mirrors `claw_fleet_core::session::TurnEnd`. */
export type TurnEnd = "clean" | "midTurn" | "interrupted" | "authFailed" | "failed";

export type SubtaskOutcome = "reported" | "midTurn" | "interrupted" | "authFailed" | "failed";

/** The outcome a stopped sub-task's row shows, or null when the row is not a
 *  stopped one (its task state already says everything). A missing
 *  `lastTurnEnd` (codex / dsh, or an unreadable tail) keeps the old
 *  "reported back" reading rather than guessing at a failure. */
export function subtaskOutcome(
  state: TaskState,
  lastTurnEnd: TurnEnd | null | undefined,
): SubtaskOutcome | null {
  if (state !== "review") return null;
  if (lastTurnEnd == null || lastTurnEnd === "clean") return "reported";
  return lastTurnEnd;
}
