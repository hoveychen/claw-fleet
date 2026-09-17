// Absolute path to the pure-chat workspace. It lives under the home directory on the **desktop host**
// and cannot be derived on mobile, so we request it from relay (mobile_relay.rs::serve_request's
// `chat_workspace`).
//
// Used in two places: the new-session modal pins it at the top of directory options (it has no
// "recent sessions" to be discovered), and the task page uses it to filter chat sessions out of
// project tasks. The desktop has a corresponding function (claw-fleet-desktop/app/hooks/useChatWorkspace.ts).
import { useEffect, useMemo, useState } from "react";
import type { FleetTransport } from "./transport";

/** `null` means we haven't fetched it yet — relay not connected, request in flight, or desktop version
 *  too old to recognize this method. Callers must treat null as "unknown", not "no chat workspace". */
export function useChatWorkspace(client: FleetTransport | null): string | null {
  const [path, setPath] = useState<string | null>(null);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<{ path: string }>("chat_workspace")
      .then((r) => {
        if (alive) setPath(r.path);
      })
      .catch(() => {
        if (alive) setPath(null);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return path;
}

/**
 * Like the above, but query **once per device**. The task page lists sessions from multiple devices,
 * and pinning the chat section at the top must hold for every machine: a remote host's chat workspace
 * is the path under its own home (e.g. `/root/.fleet/chat`), so comparing it to this machine's path
 * never matches, and that machine's Chat section sinks into the project list.
 *
 * Returns a deviceId → path map; if a device hasn't been fetched (or the desktop is too old to
 * recognize this method), it won't have a key here — callers treat that as "unknown". Fetched results
 * persist and are not re-requested on subsequent renders.
 */
export function useChatWorkspaces(
  deviceIds: readonly string[],
  clientFor: (deviceId: string) => FleetTransport | null,
): Record<string, string> {
  const [paths, setPaths] = useState<Record<string, string>>({});
  // Dependency as a joined string: callers provide a new array on each render, but device set rarely changes.
  const key = useMemo(() => [...deviceIds].sort().join(" "), [deviceIds]);
  useEffect(() => {
    if (!key) return;
    let alive = true;
    for (const id of key.split(" ")) {
      const transport = clientFor(id);
      if (!transport) continue;
      transport
        .request<{ path: string }>("chat_workspace")
        .then((r) => {
          if (alive && r?.path) setPaths((prev) => (prev[id] === r.path ? prev : { ...prev, [id]: r.path }));
        })
        .catch(() => {
          /* Older desktop versions don't recognize this method — that device just has no pinned chat, not an error. */
        });
    }
    return () => {
      alive = false;
    };
  }, [key, clientFor]);
  return paths;
}
