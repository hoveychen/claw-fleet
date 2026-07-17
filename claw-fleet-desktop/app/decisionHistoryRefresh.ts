import { listen } from "@tauri-apps/api/event";

const HISTORY_RESOLVED_EVENTS = [
  "elicitation-dismissed",
  "fleet-ask-dismissed",
  "plan-approval-dismissed",
] as const;

/**
 * Reload a mounted session's decision history after the backend removes a
 * resolved request. The MCP completion path deletes the pending request just
 * before appending its history record, so a short delay avoids racing that
 * write while keeping the conversation card visibly current.
 */
export function subscribeDecisionHistoryRefresh(
  refresh: () => void | Promise<void>,
  delayMs = 250,
): () => void {
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const unlistenPromises = HISTORY_RESOLVED_EVENTS.map((event) =>
    listen<string>(event, () => {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        if (!disposed) void refresh();
      }, delayMs);
    }),
  );

  return () => {
    disposed = true;
    if (timer !== null) clearTimeout(timer);
    for (const unlisten of unlistenPromises) {
      void unlisten.then((fn) => fn()).catch(() => {});
    }
  };
}
