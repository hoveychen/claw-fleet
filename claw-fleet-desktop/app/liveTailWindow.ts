/**
 * How wide a window the live-tail poll asks for.
 *
 * Distinct from `messageWindow.ts`, which is the *render* window over records
 * already in hand. This is the *fetch* window: the `tail` argument the
 * standalone poll in `SessionDetail` hands `get_messages_tail` every 1.5s.
 *
 * The window has to grow as the agent writes, or records the reader has already
 * scrolled back to fall off the top of a fixed-size tail. Getting that growth
 * rule wrong is expensive in a way a render window never is: every extra record
 * in the window is re-read from disk, re-serialised across the Tauri IPC
 * boundary and re-reconciled in the webview, on every single poll.
 *
 * Split out of the component so the rule can be exercised directly.
 */

import type { RawMessage } from "./types";

/** Records fetched initially, and the floor the window never drops below. */
export const LIVE_TAIL_FLOOR = 150;

/**
 * Ceiling on the fetch window.
 *
 * A live session polls every 1.5s, and this is the most work worth redoing at
 * that cadence. Reaching it means the reader has scrolled back further than the
 * poll will keep anchored for them — 「load earlier」 still reaches deeper
 * history, it just stops being re-fetched on every tick.
 */
export const LIVE_TAIL_CEILING = 1200;

/**
 * Stable id for a transcript record, matching `messageReuse`'s notion of
 * identity: uuid when the harness wrote one, otherwise the fields that together
 * pin a record to its place in the file.
 */
export function recordId(msg: RawMessage | undefined): string | null {
  if (!msg) return null;
  if (msg.uuid) return `u:${msg.uuid}`;
  const stamp = msg.timestamp ?? "";
  const msgId = msg.message?.id ?? "";
  if (!stamp && !msgId) return null;
  return `f:${msg.type}|${stamp}|${msgId}`;
}

/**
 * How many records the agent appended since the previous poll.
 *
 * Measured from the *last* record of the previous window, not the first: once
 * the window saturates, its first record slides forward and is no longer inside
 * the fetched slice at all, so it cannot be located there. The previous last
 * record still is, with the new arrivals behind it.
 *
 * Returns `next.length` when the previous tail isn't in the new window — a
 * window sharing no record with its predecessor can't say anything finer, and
 * the caller clamps the result anyway.
 */
export function arrivedSince(prevLastId: string | null, next: RawMessage[]): number {
  if (prevLastId === null || next.length === 0) return 0;
  for (let i = next.length - 1; i >= 0; i--) {
    if (recordId(next[i]) === prevLastId) return next.length - 1 - i;
  }
  return next.length;
}

export interface TailGrowth {
  /** The window this poll asked for. */
  tail: number;
  /** How many records came back. Equal to `tail` once the window saturates. */
  returned: number;
  /** New records appended since the previous poll — see `arrivedSince`. */
  arrived: number;
}

/**
 * The fetch window for the next poll.
 *
 * Grows by exactly what the transcript grew by, and only while the window is
 * saturated. That pins the window's *start* to the same record, which is the
 * entire point: nothing the reader scrolled back to slides out from under them,
 * and an idle session re-reads the same 150 records forever instead of climbing.
 *
 * What this replaces: growth used to be `tail + 1000` on every poll whose
 * `returned >= tail`. That condition holds for *any* transcript longer than the
 * window, so it never stopped — 150 → 1150 → 2150 → … until the window swallowed
 * the file, after which every tick re-read the whole transcript. Measured on a
 * 4513-record session (`claw-fleet-debug.log`, 2026-09-06 18:22): the window
 * reached the full file within ~8s of the pane opening, and each poll then took
 * 1–3s — longer than the 1.5s interval that scheduled it.
 */
export function nextLiveTail({ tail, returned, arrived }: TailGrowth): number {
  if (returned < tail) return tail;
  if (arrived <= 0) return tail;
  return Math.min(LIVE_TAIL_CEILING, tail + arrived);
}
