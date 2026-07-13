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
  const [path, setPath] = useState<string | null>(null);
  useEffect(() => {
    invoke<string>("chat_workspace")
      .then(setPath)
      .catch(() => setPath(null));
  }, []);
  return path;
}
