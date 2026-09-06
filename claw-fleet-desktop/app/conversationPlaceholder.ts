/** What the conversation pane shows when it has no messages to draw. */
export type ConversationPlaceholder = "loading" | "stalled" | null;

/**
 * Decide the placeholder for an empty conversation pane.
 *
 * Split out of MessageList so the decision is unit-testable without jsdom —
 * this is the exact branch that rendered an eternal 「加载中…」 when the backend
 * stopped answering.
 */
export function conversationPlaceholder({
  isLoading,
  stalled,
  messageCount,
}: {
  isLoading: boolean;
  stalled: boolean;
  messageCount: number;
}): ConversationPlaceholder {
  // Anything already on screen wins: `isLoading` is also raised while fetching
  // *older* history, and blanking a readable transcript for that is how the
  // "load earlier" button used to make the pane flash empty.
  if (messageCount > 0) return null;
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
