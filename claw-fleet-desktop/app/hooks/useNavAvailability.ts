import { useMemo } from "react";
import { useUIStore } from "../store";
import { isWebBuild, showsMobilePanel } from "../hostEnv";
import type { NavAvailability } from "../components/navGroups";

/** Which conditional pages this host has, for the nav rail and the section
 *  switch. One hook so the two cannot disagree about whether, say, the
 *  terminal page exists. */
export function useNavAvailability(): NavAvailability {
  // Terminal only exists if the backend started with FLEET_TERMINAL (see core's
  // feature_flags); hiding it beats letting users click in and get refused.
  const terminal = useUIStore((s) => s.hostFeatures.terminal);
  const lobsters = useUIStore((s) => s.lobsterFleetEnabled);
  // The pairing code belongs to the process that opens the relay channel; a
  // loopback-only webui has no host a phone can reach (see showsMobilePanel).
  const mobile = showsMobilePanel(isWebBuild(), window.location.protocol, window.location.hostname);
  return useMemo(() => ({ terminal, mobile, lobsters }), [terminal, mobile, lobsters]);
}
