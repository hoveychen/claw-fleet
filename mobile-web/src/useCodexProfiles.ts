// Codex profile-v2 files from the desktop host (`<CODEX_HOME>/<name>.config.toml`).
// The phone can't enumerate them—the files live on the host, and the
// `[model_providers.<id>]` block only says "how to connect", not "which models exist".
// So profiles are the only thing in codex config that can name a third-party model.
// The new-session modal uses this to augment the codex model dropdown; the selected
// value is `profile:<name>`, which codex_launch.rs's push_model_args converts to
// `codex exec -p`. The desktop equivalent is list_codex_profiles in SessionOptionPills.
import { useEffect, useState } from "react";
import type { FleetTransport } from "./transport";

export interface CodexProfile {
  name: string;
  model: string | null;
  model_provider: string | null;
  reasoning_effort: string | null;
}

/** Returns an empty array if unreachable (relay not connected, request in flight,
 *  or desktop version too old to know this method). We intentionally don't use null
 *  to distinguish "unknown"—callers just append to the built-in model list, and
 *  an empty array's fallback behavior (show only official models) is what we want. */
export function useCodexProfiles(client: FleetTransport | null): CodexProfile[] {
  const [profiles, setProfiles] = useState<CodexProfile[]>([]);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<CodexProfile[]>("codex_profiles")
      .then((r) => {
        if (alive) setProfiles(r ?? []);
      })
      .catch(() => {
        if (alive) setProfiles([]);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return profiles;
}

/** profile → model dropdown entry `[value, label]`. Label prefers the profile's own
 *  model id (what the user recognizes), falling back to the profile name if model isn't set. */
export function codexProfileChoices(
  profiles: CodexProfile[],
): Array<[string, string]> {
  return profiles.map((p) => {
    const model = p.model?.trim();
    const provider = p.model_provider?.trim();
    const label = model ? (provider ? `${model} (${provider})` : model) : p.name;
    return [`profile:${p.name}`, label];
  });
}
