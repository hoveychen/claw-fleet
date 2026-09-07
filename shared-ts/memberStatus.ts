/**
 * How to read a session-family member's status when the question is "is this
 * one still doing something" — shared by the desktop (`types.ts`) and the
 * mobile web client so the two cannot drift on it.
 *
 * `waitingInput` is derived from the transcript shape (core
 * `session/detect.rs`): the last assistant message carried
 * `stop_reason=end_turn` less than 300s ago. On a main session that genuinely
 * means "parked, your turn". A subagent has no user to answer it — its
 * `end_turn` is the final report going back to the parent, i.e. it is *done*.
 * Reported unchanged, a finished Explore agent sat in a panel titled 运行中的
 * Agent wearing 等待输入 for the whole 300s window before aging out to Idle.
 *
 * A subagent parked on a decision card is unaffected: an outstanding MCP call
 * leaves `stop_reason=tool_use`, which `determine_status` maps to Executing,
 * never to `waitingInput`.
 *
 * Core already agrees — `running_subagent_count` (session/scan.rs) counts only
 * Thinking/Executing/Streaming/Delegating/Processing. This is the client-side
 * half of that same rule.
 */
export function memberDisplayStatus<S extends string>(member: {
  isSubagent: boolean;
  status: S;
}): S | "idle" {
  if (member.isSubagent && member.status === "waitingInput") return "idle";
  return member.status;
}
