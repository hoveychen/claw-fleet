/**
 * How hard a task is generating right now, as the tier its row's state mark
 * animates at. Shared by the desktop task list and the phone so the same task
 * pulses the same way on both.
 *
 * The input is the task's total output speed (tok/s): the latest main hop's
 * `agentTokenSpeed` plus the latest hop of every sub-task, each of which
 * already folds in its own subagents. The speed is a 5-minute window measured
 * against "now" (`session/stats.rs::finish_at`), so it eases back to 0 once
 * generation stops and the mark settles with it.
 *
 * Colour stays the task's state; the tier only drives motion. A running task
 * at 0 tok/s is waiting on a tool, not generating, and its mark holds still.
 */
export type SpeedTier = 0 | 1 | 2 | 3;

/** Lower bound (tok/s) of tiers 1, 2 and 3. Measured 2026-10-19 on the dev
 *  box: one Opus session generates ~20–70 tok/s, so tier 2 is "one session
 *  going flat out" and tier 3 is only reached by a task with helpers fanned
 *  out in parallel. */
export const SPEED_TIER_FLOORS: readonly [number, number, number] = [1, 40, 150];

export function speedTier(tokPerSec: number | null | undefined): SpeedTier {
  const v = tokPerSec ?? 0;
  if (!(v >= SPEED_TIER_FLOORS[0])) return 0;
  if (v < SPEED_TIER_FLOORS[1]) return 1;
  if (v < SPEED_TIER_FLOORS[2]) return 2;
  return 3;
}

/** Sum the speeds of the sessions that make up a task (one per conversation
 *  line: the main line's tip and each sub-task's tip). */
export function sumAgentSpeed(
  tips: ReadonlyArray<{ agentTokenSpeed?: number | null } | null | undefined>,
): number {
  return tips.reduce((n, s) => n + (s?.agentTokenSpeed ?? 0), 0);
}
