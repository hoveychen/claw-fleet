/**
 * Which workspaces run on another machine through rca.
 *
 * The registry is small, changes only when the user registers or removes a
 * workspace, and is needed by three unrelated surfaces at once (session card,
 * session list, tab strip). A per-component fetch would mean one IPC round trip
 * per card on every board render, so it lives in one store, is fetched once,
 * and is refreshed explicitly by whoever mutates it.
 *
 * The lookup is prefix-aware on purpose: rca routes anything at or under a
 * registered path (`remote_workspace::find_for_path` uses the same
 * equal-or-under rule), so a session started in a subdirectory of a registered
 * workspace is just as remote as one started at its root. Matching only exact
 * paths would badge the parent and silently miss every child.
 */

import { useEffect } from "react";
import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { RemoteWorkspace, RemoteWorkspacesConfig } from "../types";

interface RemoteWorkspacesState {
  workspaces: RemoteWorkspace[];
  loaded: boolean;
  refresh: () => Promise<void>;
}

export const useRemoteWorkspacesStore = create<RemoteWorkspacesState>((set) => ({
  workspaces: [],
  loaded: false,
  refresh: async () => {
    try {
      const cfg = await invoke<RemoteWorkspacesConfig>("list_remote_workspaces");
      set({ workspaces: cfg.workspaces ?? [], loaded: true });
    } catch {
      // A backend that cannot answer means "nothing is remote" for badging
      // purposes — the board must still render.
      set({ loaded: true });
    }
  },
}));

/** Is `path` at, or under, a registered remote workspace? */
export function findRemoteWorkspace(
  workspaces: RemoteWorkspace[],
  path: string | null | undefined,
): RemoteWorkspace | undefined {
  if (!path) return undefined;
  const clean = path.replace(/\/+$/, "");
  return workspaces.find((w) => {
    const root = w.path.replace(/\/+$/, "");
    // `startsWith` alone would match `/srv/repo-old` against `/srv/repo`; the
    // separator check is what keeps a sibling from being badged as a child.
    return clean === root || clean.startsWith(`${root}/`);
  });
}

/**
 * Fetch the registry once for the whole app. Mount this near the root; every
 * other consumer just reads the store.
 */
export function useRemoteWorkspacesSync(): void {
  const loaded = useRemoteWorkspacesStore((s) => s.loaded);
  const refresh = useRemoteWorkspacesStore((s) => s.refresh);
  useEffect(() => {
    if (!loaded) void refresh();
  }, [loaded, refresh]);
}
