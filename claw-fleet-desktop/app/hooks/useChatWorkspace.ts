import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * Absolute path of the pure-chat workspace, or `null` until it resolves (and
 * for good if the backend can't hand one back).
 *
 * It comes from the backend rather than being rebuilt in the UI because under a
 * remote connection the chat workspace lives in the probe host's home, not on
 * this machine — see `Backend::chat_workspace`.
 */
export function useChatWorkspace(): string | null {
  return useChatWorkspaceState().path;
}

/**
 * The chat workspace plus whether the backend has answered. `loaded` turns
 * true on success and on failure, so `path === null && loaded` means "this host
 * has no chat workspace" rather than "still asking".
 */
export function useChatWorkspaceState(): { path: string | null; loaded: boolean } {
  const [state, setState] = useState<{ path: string | null; loaded: boolean }>({
    path: null,
    loaded: false,
  });
  useEffect(() => {
    let live = true;
    invoke<string>("chat_workspace")
      .then((p) => {
        if (live) setState({ path: p ?? null, loaded: true });
      })
      .catch(() => {
        if (live) setState({ path: null, loaded: true });
      });
    return () => {
      live = false;
    };
  }, []);
  return state;
}
