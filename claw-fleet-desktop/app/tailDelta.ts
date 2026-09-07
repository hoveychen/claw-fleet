/**
 * Merging a live-tail delta into the transcript already on screen.
 *
 * Two callers follow a running session and both need the same rule, so it lives
 * here rather than inline in either: the global store's `session-tail` listener
 * (pushed by the desktop watcher) and `SessionDetail`'s standalone poll.
 *
 * Deduping is not belt-and-braces. Claude's delta is a byte-offset slice and
 * never overlaps, so it is a no-op there — but Codex's live follow re-normalizes
 * a trailing *window* on every poll (its rollout is folded; see
 * `CodexSource::tail_incremental`), so consecutive pushes do overlap, and the
 * event→response_item swap re-emits the same reply under its stable uuid.
 * Records without a uuid are always kept: there is nothing to key on, and
 * dropping them would silently lose rows.
 */

import type { RawMessage } from "./types";

/**
 * `prev` with everything in `incoming` that it doesn't already hold.
 *
 * Returns `prev` itself when the delta brought nothing new, so consumers
 * memoised on the array short-circuit instead of re-rendering the transcript.
 */
export function appendTailDelta(prev: RawMessage[], incoming: RawMessage[]): RawMessage[] {
  if (incoming.length === 0) return prev;
  const seen = new Set(prev.map((m) => m.uuid).filter((u): u is string => !!u));
  const fresh = incoming.filter((m) => !m.uuid || !seen.has(m.uuid));
  return fresh.length > 0 ? [...prev, ...fresh] : prev;
}
