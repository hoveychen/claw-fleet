import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";

/**
 * Shared state for the system keep-awake toggle (caffeinate -i equivalent).
 *
 * The Rust side (keep_awake.rs) is the single source of truth: it holds the
 * OS power assertion and persists the flag to ~/.fleet/keep-awake.json. This
 * hook mirrors that state and listens for `keep-awake-changed` so the main
 * window's toolbar button and the standalone settings window stay in sync.
 */
export function useKeepAwake() {
  const [enabled, setEnabled] = useState(false);
  const [supported, setSupported] = useState(false);

  useEffect(() => {
    invoke<boolean>("keep_awake_supported").then(setSupported).catch(() => {});
    invoke<boolean>("get_keep_awake").then(setEnabled).catch(() => {});
    const unlisten = listen<boolean>("keep-awake-changed", (e) =>
      setEnabled(!!e.payload),
    );
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  const setKeepAwake = useCallback(async (next: boolean) => {
    setEnabled(next); // optimistic; reverted on failure
    try {
      await invoke<boolean>("set_keep_awake", { enabled: next });
    } catch (e) {
      console.error("set_keep_awake failed:", e);
      setEnabled(!next);
    }
  }, []);

  return { enabled, supported, setKeepAwake };
}
