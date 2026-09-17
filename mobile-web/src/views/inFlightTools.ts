import { createContext, useContext } from "react";
import type { ContentBlock, RawMessage } from "../types";

/**
 * `tool_use` ids whose tool is executing *right now* — the mobile counterpart of
 * the desktop `blocks/inFlightTools.ts`.
 *
 * The transcript gives no direct signal: Claude Code finalises the assistant
 * record (`stop_reason: "tool_use"`) the moment the model asks for a tool and
 * appends the `tool_result` only when the tool returns, minutes later for a long
 * `Bash`. In between, a tool step rendered exactly like a finished one. The band
 * above it already knew (it withholds its Done check), but the step itself said
 * nothing.
 */
export const InFlightToolsContext = createContext<ReadonlySet<string>>(new Set());

export function useInFlightTools(): ReadonlySet<string> {
  return useContext(InFlightToolsContext);
}

/**
 * The tool calls of the newest assistant record that have no `tool_result` yet.
 *
 * Only that record can hold live calls — the next assistant record is written
 * only once every result of the previous one has landed. Earlier records missing
 * a result lost it to the snapshot's trimming, and labelling those "running"
 * would be a lie that never clears. Same reason the set is empty unless the
 * session is in a working status: a killed turn leaves this shape behind
 * forever.
 */
export function inFlightToolIds(
  messages: RawMessage[],
  resultIds: ReadonlySet<string>,
  working: boolean,
  blocksOf: (msg: RawMessage) => ContentBlock[],
): Set<string> {
  const ids = new Set<string>();
  if (!working) return ids;
  const last = [...messages].reverse().find((m) => m.type === "assistant");
  if (!last) return ids;
  for (const b of blocksOf(last)) {
    if (b.type === "tool_use" && b.id && !resultIds.has(b.id)) ids.add(b.id);
  }
  return ids;
}

/** A `Bash` the agent launched with `run_in_background`: its result lands
 *  instantly and the turn ends (the session flips to "awaiting input") while the command
 *  keeps running. Nothing on screen used to say so. */
export function isBackgroundShell(b: ContentBlock): boolean {
  if (b.name !== "Bash") return false;
  const input = b.input as { run_in_background?: unknown } | undefined;
  return input?.run_in_background === true;
}
