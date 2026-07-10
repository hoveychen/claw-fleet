/**
 * The render window over a transcript.
 *
 * `MessageList` renders only the trailing slice of a session so the DOM stays
 * bounded — a session's median length is ~213 messages and the tail reaches
 * 2000. The window is expressed as a **count from the end**, never as a start
 * index, because the store prepends older history on demand: a start index
 * would silently point at a different message after every prepend, while a
 * count-from-the-end leaves the visible set untouched.
 *
 * Split out of the component so these invariants can be exercised directly.
 */

/** The slice of `[0, length)` that is currently rendered. */
export interface WindowSlice {
  /** Index of the first rendered message. */
  start: number;
  /** How many messages sit above the window, still unrendered. */
  hidden: number;
}

export function windowSlice(length: number, visibleCount: number): WindowSlice {
  const start = Math.max(0, length - visibleCount);
  return { start, hidden: start };
}

/** What changed about the message array since the previous render. */
export interface WindowGrowth {
  /** True when older history was prepended (a "load earlier" landed). */
  prepended: boolean;
  /** Net change in message count. Negative on a session switch. */
  delta: number;
  /** Current window size. */
  visibleCount: number;
  /** The default window size; above it, the reader has expanded the window. */
  pageSize: number;
}

/**
 * The window size after the transcript changed.
 *
 * - **Prepend**: unchanged. Counting from the end already pins the visible set.
 * - **Append while at the default size**: unchanged, so the window slides
 *   forward and drops the oldest row — correct while the reader sits at the
 *   bottom watching the agent write.
 * - **Append after the reader expanded the window**: grow by the same delta, so
 *   the window's start stays pinned and nothing is yanked out from under them.
 */
export function nextVisibleCount({
  prepended,
  delta,
  visibleCount,
  pageSize,
}: WindowGrowth): number {
  const expanded = visibleCount > pageSize;
  if (prepended || delta <= 0 || !expanded) return visibleCount;
  return visibleCount + delta;
}

/**
 * Window size needed to bring `matchIndex` into view with some context above it.
 * Returns at least `pageSize`, so a match near the tail doesn't shrink the window.
 */
export function visibleCountForMatch(
  length: number,
  matchIndex: number,
  pageSize: number,
  context = 10,
): number {
  return Math.max(pageSize, length - matchIndex + context);
}
