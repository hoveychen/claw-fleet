// Monitored agent sources on the desktop host ({name, enabled, available}). Mobile can't derive
// "installed/enabled", so it fetches from relay (mobile_relay.rs::serve_request's `sources_config`).
// The new session modal uses this to restrict the tool picker to actually-monitored sources—
// when a Codex source is disabled, it shouldn't list Codex in the launcher. The desktop equivalent
// is `get_sources_config` in claw-fleet-desktop/app/components/SettingsPanel.
import { useEffect, useState } from "react";
import type { FleetTransport } from "./transport";

export interface SourceInfo {
  name: string;
  enabled: boolean;
  available: boolean;
}

/** `null` means not yet fetched—relay disconnected, request in flight, or desktop version too old to recognize this method.
 *  Callers must treat null as "don't know", not "no sources". */
export function useSourcesConfig(client: FleetTransport | null): SourceInfo[] | null {
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<SourceInfo[]>("sources_config")
      .then((r) => {
        if (alive) setSources(r);
      })
      .catch(() => {
        if (alive) setSources(null);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return sources;
}
