import { createContext, useContext } from "react";
import type { RawMessage, ToolResultBlock } from "../../types";

/**
 * `tool_use` ids whose tool is executing *right now*.
 *
 * A card only knew it was running while its assistant record was still
 * streaming (`stop_reason === null`). But the real execution window opens right
 * after that: Claude Code finalises the assistant record with
 * `stop_reason: "tool_use"` and only appends the `tool_result` when the tool
 * returns — minutes later for a long `Bash`. In that window a card had neither
 * spinner nor output, which is pixel-identical to "finished, output collapsed".
 *
 * So MessageList publishes the in-flight ids here and the cards read them. The
 * set is empty unless the scanner says the agent is working, because a missing
 * `tool_result` on a dead session means the turn was killed, not that something
 * is still running.
 */
export const InFlightToolsContext = createContext<ReadonlySet<string>>(new Set());

export function useInFlightTools(): ReadonlySet<string> {
  return useContext(InFlightToolsContext);
}

/**
 * The tool calls of the newest assistant record that have no `tool_result` yet.
 *
 * Only that record can hold live calls: Claude Code writes the next assistant
 * record only after every result of the previous one has landed. Earlier
 * records missing a result were trimmed for transport or lost to windowing —
 * labelling those "running" would be a lie that never clears.
 */
export function inFlightToolIds(
  messages: RawMessage[],
  resultMap: Map<string, ToolResultBlock>,
  working: boolean,
): Set<string> {
  const ids = new Set<string>();
  if (!working) return ids;
  const last = [...messages].reverse().find((m) => m.type === "assistant");
  const content = last?.message?.content;
  if (!Array.isArray(content)) return ids;
  for (const block of content) {
    if (block.type !== "tool_use") continue;
    const id = (block as { id?: string }).id;
    if (id && !resultMap.has(id)) ids.add(id);
  }
  return ids;
}
