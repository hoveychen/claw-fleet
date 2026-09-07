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

/** Records fetched initially, and how much the window used to grow by. */
export const LIVE_TAIL_FLOOR = 150;
export const LIVE_TAIL_STEP = 1000;

export interface TailGrowth {
  /** The window this poll asked for. */
  tail: number;
  /** How many records came back. Equal to `tail` once the window saturates. */
  returned: number;
}

/**
 * The fetch window for the next poll.
 *
 * A saturated window means the next transcript write would slide an
 * already-rendered record out of the top, so the window grows to keep the
 * visible history anchored.
 */
export function nextLiveTail({ tail, returned }: TailGrowth): number {
  if (returned >= tail) return tail + LIVE_TAIL_STEP;
  return tail;
}
