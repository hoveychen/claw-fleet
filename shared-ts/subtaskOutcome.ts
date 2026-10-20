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

/** `takenOver` is never returned by `subtaskOutcome` itself; see
 *  `subtaskOutcomeOnStep`. */
export type SubtaskOutcome =
  | "reported"
  | "midTurn"
  | "interrupted"
  | "authFailed"
  | "failed"
  | "takenOver";

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

/** The outcome of a sub-task dispatched for a plan step. A stop that was not a
 *  clean report back stops being an alarm once the work moved on without it:
 *  the step it was dispatched for is checked off, or a sub-task dispatched
 *  later for the same step exists. Its row then reads "taken over" rather than
 *  as a live failure, which made a waiting task look stuck on it. */
export function subtaskOutcomeOnStep(
  sub: { state: TaskState; lastTurnEnd: TurnEnd | null | undefined; createdAtMs: number },
  step: { done: boolean; siblingCreatedAtMs: readonly number[] },
): SubtaskOutcome | null {
  const outcome = subtaskOutcome(sub.state, sub.lastTurnEnd);
  if (outcome == null || outcome === "reported") return outcome;
  if (step.done || step.siblingCreatedAtMs.some((ms) => ms > sub.createdAtMs)) return "takenOver";
  return outcome;
}

/** Whether a sub-task counts toward a task's "n/m finished" tally: it is done,
 *  or it stopped having reported back. A stopped one that was cut off, killed
 *  or failed (`lastTurnEnd` set and not "clean") stays in the total but is not
 *  finished. A missing `lastTurnEnd` keeps the old reading. */
export function subtaskCountsAsFinished(
  state: TaskState,
  lastTurnEnd: TurnEnd | null | undefined,
): boolean {
  if (state === "done") return true;
  return subtaskOutcome(state, lastTurnEnd) === "reported";
}
