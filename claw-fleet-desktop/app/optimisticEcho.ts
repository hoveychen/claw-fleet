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
 * preamble — and the list folds a run of them into one collapsed "System Context" card
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

/**
 * Whether a send earns an immediate bubble.
 *
 * A resume does: the agent is cold-starting and will not write the record for
 * seconds. An enqueue does too — but only when it was *injected* into the live
 * turn, because the receiving CLI does not write its record until it absorbs
 * the message at its next tool boundary, which on a long tool call is minutes
 * away. A genuinely queued message has not been delivered at all, and its
 * honest affordance is the "queued" chip, not a bubble.
 */
export function shouldEchoSend(
  mode: "resume" | "enqueue",
  delivery?: "injected" | "queued",
): boolean {
  return mode === "resume" || delivery === "injected";
}

/** Whether one echoed text is still waiting for its real transcript row. */
export function stillPending(text: string, landed: Set<string>): boolean {
  return !landed.has(text.trim());
}
