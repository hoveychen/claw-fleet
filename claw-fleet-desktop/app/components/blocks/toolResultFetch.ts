import { createContext, useContext, useEffect, useState } from "react";

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

/**
 * Recover the full, untrimmed output for a truncated tool card, once and only
 * when the reader has actually expanded it.
 *
 * The fetch is deferred until `open` so a collapsed card never pays for output
 * the reader may never look at. `loadingFull` drives the "Loading full output…"
 * placeholder; `full` is the recovered body that swaps in when it lands.
 *
 * Note `loadingFull` is deliberately NOT an effect dependency: setting it fires
 * a re-render, and if the effect re-ran on that change its own cleanup would
 * cancel the in-flight fetch before it could resolve — leaving the card stuck
 * on the placeholder forever. The effect keys only on the inputs that should
 * actually restart a fetch (open / truncated / fetcher / id).
 */
export function useFullToolResult(
  open: boolean,
  wasTruncated: boolean,
  toolFetch: ToolResultFetch | null,
  blockId: string | undefined,
): { full: FullToolResult | null; loadingFull: boolean } {
  const [full, setFull] = useState<FullToolResult | null>(null);
  const [loadingFull, setLoadingFull] = useState(false);
  useEffect(() => {
    if (!open || !wasTruncated || full || !toolFetch || !blockId) return;
    let cancelled = false;
    setLoadingFull(true);
    toolFetch
      .fetchFull(blockId)
      .then((r) => {
        if (!cancelled) setFull(r);
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setLoadingFull(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, wasTruncated, full, toolFetch, blockId]);
  return { full, loadingFull };
}
