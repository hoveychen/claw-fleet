/**
 * When a locally-echoed send has landed in the real transcript.
 *
 * A submit paints the user's text as a bubble immediately, because the agent
 * takes seconds to cold-start and write the record. The echo is retired once its
 * text shows up as a real user row — so "which rows count as the user speaking"
 * decides whether the send stays visible or disappears.
 */

import type { RawMessage } from "./types";
import { messageToText } from "./messageRows";

/**
 * Trimmed text of every transcript row that renders as the *user's own bubble*.
 *
 * `isMeta` rows are excluded. They are harness injections that happen to be
 * stored as user records — dsh writes its agent-instructions, its runtime
 * snapshot and every Fleet guidance block as `user/message`, codex has its own
 * preamble — and the list folds a run of them into one collapsed 系统上下文 card
 * rather than a bubble. Treating one as "the prompt landed" retires the echo
 * against a row the reader cannot see, and the send vanishes from the
 * conversation with nothing left in its place.
 */
export function landedUserTexts(messages: RawMessage[]): Set<string> {
  const set = new Set<string>();
  for (const m of messages) {
    if (m.type === "user" && !m.isMeta) set.add(messageToText(m).trim());
  }
  return set;
}

/** Whether one echoed text is still waiting for its real transcript row. */
export function stillPending(text: string, landed: Set<string>): boolean {
  return !landed.has(text.trim());
}
