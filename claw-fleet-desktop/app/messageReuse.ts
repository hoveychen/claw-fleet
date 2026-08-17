/**
 * Keep object identity across a re-fetched transcript tail.
 *
 * The live tail poll re-reads the whole window every 1.5s and hands React a
 * brand-new array of brand-new objects — even when not a single message
 * changed. Every `useMemo` keyed on the array recomputes, every memoised
 * `MessageRow` re-renders, and the markdown and syntax highlighting in all of
 * them is rebuilt from scratch. On a long transcript that is a full re-render
 * of the conversation several dozen times a minute, for nothing.
 *
 * Transcripts are append-only, so a record that is already on disk will not
 * change. That makes identity cheap to preserve: match by id, reuse the object
 * we already have, and when nothing at all changed hand back the *same array*,
 * which stops the recompute at the very top.
 */

import type { RawMessage } from "./types";

/**
 * Stable identity for a transcript record.
 *
 * `uuid` is present on Claude Code transcripts. The normalised codex/dsh
 * records don't always carry one, so fall back to the fields that together pin
 * a record to its place in the file. Returns null when there is nothing stable
 * to match on, in which case the record is never reused.
 */
function identity(m: RawMessage): string | null {
  if (m.uuid) return `u:${m.uuid}`;
  const stamp = m.timestamp ?? "";
  const msgId = m.message?.id ?? "";
  if (!stamp && !msgId) return null;
  return `f:${m.type}|${stamp}|${msgId}`;
}

/** Deep-equality via serialisation. Only ever applied to a single record. */
function sameContent(a: RawMessage, b: RawMessage): boolean {
  try {
    return JSON.stringify(a) === JSON.stringify(b);
  } catch {
    return false;
  }
}

/**
 * Merge a freshly-fetched tail into what is already rendered.
 *
 * Returns `prev` itself when the fetch brought nothing new, so every consumer
 * memoised on the array short-circuits. Otherwise returns a new array whose
 * unchanged entries are the previous objects, so memoised rows still hit.
 */
export function reconcileMessages(prev: RawMessage[], next: RawMessage[]): RawMessage[] {
  if (prev === next) return prev;
  if (prev.length === 0 || next.length === 0) return next;

  const byId = new Map<string, RawMessage>();
  for (const m of prev) {
    const id = identity(m);
    if (id !== null) byId.set(id, m);
  }

  const merged: RawMessage[] = new Array(next.length);
  let allReused = true;
  for (let i = 0; i < next.length; i++) {
    const fresh = next[i];
    const id = identity(fresh);
    const old = id === null ? undefined : byId.get(id);
    // The last record is the one a writer could still be revising, so it is the
    // only one worth paying a deep comparison for. Everything above it is
    // settled history and is reused on an id match alone.
    const reusable =
      old !== undefined && (i < next.length - 1 || sameContent(old, fresh));
    if (reusable) {
      merged[i] = old;
    } else {
      merged[i] = fresh;
      allReused = false;
    }
  }

  // Same records, same order, same count: hand back the original array so
  // nothing downstream recomputes at all.
  if (allReused && prev.length === next.length) {
    for (let i = 0; i < prev.length; i++) {
      if (prev[i] !== merged[i]) return merged;
    }
    return prev;
  }
  return merged;
}
