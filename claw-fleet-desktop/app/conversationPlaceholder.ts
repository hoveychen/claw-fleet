/** What the conversation pane shows when it has no messages to draw. */
export type ConversationPlaceholder = "loading" | "stalled" | "failed" | null;

/**
 * Decide the placeholder for an empty conversation pane.
 *
 * Split out of MessageList so the decision is unit-testable without jsdom —
 * this is the exact branch that rendered an eternal 「加载中…」 when the backend
 * stopped answering.
 *
 * `stalled` and `failed` are deliberately separate. Both used to raise the same
 * flag, so a fetch that *rejected in milliseconds* — a bad path, a source that
 * cannot handle the URI — told the reader "加载超时——后端一直没有响应", which
 * is not merely unhelpful but points the next debugger at the wrong half of the
 * system. A rejection knows why it failed; say that instead.
 */
export function conversationPlaceholder({
  isLoading,
  stalled,
  failed = false,
  messageCount,
}: {
  isLoading: boolean;
  stalled: boolean;
  /** The fetch rejected. Wins over `stalled`: an error is the more specific
   *  answer, and a slow fetch that eventually rejects raises both. */
  failed?: boolean;
  messageCount: number;
}): ConversationPlaceholder {
  // Anything already on screen wins: `isLoading` is also raised while fetching
  // *older* history, and blanking a readable transcript for that is how the
  // "load earlier" button used to make the pane flash empty.
  if (messageCount > 0) return null;
  if (failed) return "failed";
  if (stalled) return "stalled";
  if (isLoading) return "loading";
  return null;
}

/** Whether a populated transcript needs a visible "catching up" marker. */
export function showLatestSync({
  isLoading,
  isLoadingEarlier,
  messageCount,
}: {
  isLoading: boolean;
  isLoadingEarlier: boolean;
  messageCount: number;
}): boolean {
  return isLoading && !isLoadingEarlier && messageCount > 0;
}
