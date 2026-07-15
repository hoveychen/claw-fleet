import { createContext, useContext } from "react";

/** Full tool output recovered on demand from the backend for one tool call. */
export interface FullToolResult {
  /** The untrimmed `tool_result` block content (string or content-block array). */
  content: unknown;
  /** The untrimmed sibling `toolUseResult`, or null when the call had none. */
  toolUseResult: unknown;
}

/**
 * Lets a tool card recover the full output the tail payload truncated.
 *
 * `MessageList` provides this when it knows the session's `jsonlPath`. A card
 * whose `tool_use_id` is in `truncatedIds` shipped only a preview (see the Rust
 * `message_trim`); on expand it calls `fetchFull` to pull the real body. Absent
 * (context is null) when nothing was truncated or the caller has no jsonl path,
 * in which case cards render whatever inline content they were given.
 */
export interface ToolResultFetch {
  truncatedIds: Set<string>;
  fetchFull: (toolUseId: string) => Promise<FullToolResult>;
}

export const ToolResultFetchContext = createContext<ToolResultFetch | null>(null);

export function useToolResultFetch(): ToolResultFetch | null {
  return useContext(ToolResultFetchContext);
}
