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

/** How long a trailing user prompt may sit unanswered before it stops reading
 *  as a resume gap. The gap itself is seconds; a genuinely in-flight turn keeps
 *  the scanner on a working status the whole time (codex's minutes-long tool
 *  silences included — `turn_in_flight` covers those). So a prompt this old
 *  with an idle scanner means the turn died without leaving a record. Matches
 *  the backend's own 300s turn-boundary freshness window. */
const RESUME_GAP_MAX_MS = 5 * 60_000;

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
 *   for input, so show the working indicator (this is what the reader expects:
 *   "it's thinking", not "等待输入") — but only while the prompt is *fresh*. A
 *   turn that died without writing any terminal record (a killed codex, a
 *   crashed harness) leaves that prompt at the tail forever, and an unbounded
 *   resume gap turned that into a 「处理中…」 spinner that never cleared.
 * - Otherwise, an assistant turn that closed with `end_turn` and nothing after
 *   it is a genuine wait for the user.
 *
 * `nowMs` is injectable for tests; it defaults to the wall clock.
 */
export function trailingIndicator(
  messages: RawMessage[],
  status?: string | null,
  nowMs: number = Date.now(),
): TrailingIndicator {
  if (status && WORKING_STATUSES.has(status)) return "working";
  const last = messages[messages.length - 1];
  if (!last) return null;
  // A fresh user prompt sitting at the tail (not an injected meta/system turn,
  // not a compact summary) is a turn about to run — the resume gap. Never read
  // it as "waiting for input".
  if (last.type === "user" && !last.isMeta && !last.isCompactSummary) {
    // An unparseable / absent timestamp keeps the old unconditional behaviour —
    // better a stale spinner than swallowing a live turn's indicator.
    const startedMs = last.timestamp ? Date.parse(last.timestamp) : NaN;
    const stale = Number.isFinite(startedMs) && nowMs - startedMs > RESUME_GAP_MAX_MS;
    return stale ? "waiting" : "working";
  }
  const lastAssistant = [...messages].reverse().find((m) => m.type === "assistant");
  if (lastAssistant?.message?.stop_reason === "end_turn") return "waiting";
  return null;
}
