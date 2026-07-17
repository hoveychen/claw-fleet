import { createContext, useContext, useEffect, useRef, useState } from "react";

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
 * on the placeholder forever.
 *
 * `toolFetch` is deliberately NOT a dependency either, for the same reason at a
 * larger scale. On a live session every `session-tail` append rebuilds
 * MessageList's `truncatedIds` Set, which hands this hook a brand-new
 * `toolFetch` object identity even though nothing about the fetch changed. If
 * that identity were a dep, its cleanup would cancel the in-flight
 * `get_tool_result_full` read (a multi-MB jsonl scan) on every append — and on
 * a busy session the appends out-pace the read, so the fetch is cancelled and
 * restarted forever and the recovered body never lands (an expanded image Read
 * shows the file path and an empty body). We read the latest fetcher through a
 * ref instead, and key the effect only on the inputs that should actually
 * (re)start a fetch: open / truncated / id. `wasTruncated` already flips false→
 * true when `toolFetch` first becomes available, so that transition still
 * triggers the initial fetch.
 */
export function useFullToolResult(
  open: boolean,
  wasTruncated: boolean,
  toolFetch: ToolResultFetch | null,
  blockId: string | undefined,
): { full: FullToolResult | null; loadingFull: boolean; error: string | null } {
  const [full, setFull] = useState<FullToolResult | null>(null);
  const [loadingFull, setLoadingFull] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const toolFetchRef = useRef(toolFetch);
  toolFetchRef.current = toolFetch;
  useEffect(() => {
    const fetcher = toolFetchRef.current;
    if (!open || !wasTruncated || full || !fetcher || !blockId) return;
    let cancelled = false;
    setLoadingFull(true);
    setError(null);
    fetcher
      .fetchFull(blockId)
      .then((r) => {
        if (!cancelled) setFull(r);
      })
      // Surface the reason instead of swallowing it: a silently-empty card gave
      // no signal whether the recovery fired at all or failed mid-flight.
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoadingFull(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, wasTruncated, full, blockId]);
  return { full, loadingFull, error };
}
