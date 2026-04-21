import { emit, listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useDecisionStore } from "../store";

export const DECISION_PEER_DISMISS = "decision-peer-dismiss";

/**
 * Keep the decision store in sync between the main window and the floating
 * decision window. When one window responds to (and locally removes) a
 * decision, it emits `decision-peer-dismiss` with the id so the other
 * window drops its stale copy.
 *
 * Safe to mount twice: `dismiss` is idempotent, so receiving your own
 * broadcast is a no-op if the id is already gone.
 */
export function useDecisionPeerSync() {
  useEffect(() => {
    const unlisten = listen<string>(DECISION_PEER_DISMISS, (e) => {
      const id = e.payload;
      if (!id) return;
      useDecisionStore.getState().dismiss(id);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);
}

export function broadcastDecisionDismissed(id: string) {
  emit(DECISION_PEER_DISMISS, id).catch((e) => {
    console.warn("[decision-peer-sync] emit failed:", e);
  });
}
