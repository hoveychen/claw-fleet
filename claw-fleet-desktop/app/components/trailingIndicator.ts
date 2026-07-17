import type { RawMessage } from "../types";

/** Session statuses that mean the agent is actively doing something right now
 *  (as opposed to waiting for the user). Mirrors the scanner's working set. */
export const WORKING_STATUSES = new Set([
  "thinking",
  "executing",
  "streaming",
  "processing",
  "active",
  "delegating",
]);

export type TrailingIndicator = "working" | "waiting" | null;

/**
 * Which activity indicator, if any, to pin under the last message.
 *
 * `messages` is the already-renderable-filtered tail (tool_result-only user
 * rows are dropped upstream). `status` is the live scanner status.
 *
 * - When the scanner says the agent is working, always show the working dots.
 * - A trailing genuine *user* turn means a new prompt has landed but the
 *   agent's first block hasn't yet — the resume gap. The scanner status lags a
 *   scan cycle behind a fresh submit, and Opus omits the live-thinking sidecar,
 *   so there is nothing else to show. That is a turn *about to run*, NOT a wait
 *   for input, so show the working indicator (this is what the reader expects:
 *   "it's thinking", not "等待输入").
 * - Otherwise, an assistant turn that closed with `end_turn` and nothing after
 *   it is a genuine wait for the user.
 */
export function trailingIndicator(
  messages: RawMessage[],
  status?: string | null,
): TrailingIndicator {
  if (status && WORKING_STATUSES.has(status)) return "working";
  const last = messages[messages.length - 1];
  if (!last) return null;
  // A fresh user prompt sitting at the tail (not an injected meta/system turn,
  // not a compact summary) is a turn about to run — the resume gap. Never read
  // it as "waiting for input".
  if (last.type === "user" && !last.isMeta && !last.isCompactSummary) return "working";
  const lastAssistant = [...messages].reverse().find((m) => m.type === "assistant");
  if (lastAssistant?.message?.stop_reason === "end_turn") return "waiting";
  return null;
}
