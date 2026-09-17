// "Which device am I looking at right now?" — device scope for UI to read.
//
// Why context and not a prop: what needs it isn't one or two places, it's
// local persistence scattered throughout — new session drafts, attachments,
// last-used repo, resume input, task page workspace filter. These keys used to
// be global, fine in the single-device era; post multi-device they're all
// "things that belong to a specific machine". A workspace path on machine A
// doesn't exist on B, and session ids are unique only per machine, so keys like
// `resume:<id>` would collide across devices.
//
// A second reason for context: it's right for the next phase. After inbox
// aggregation, drilling into a detail page from a merged list belongs to **that
// device**, not the current scope's device. That drill needs just one more
// Provider wrapping its piece, keyed to the belonging device, and all drafts
// inside automatically land in the right namespace. A prop or module global
// can't do this (the latter silently reads wrong when two devices are both
// present).
//
// Division of labor with the transport layer: transport asks "where does data
// come from", this asks "where does local storage write to". Transport still
// uses props (see transport.ts for the seam).

import { createContext, useContext, type ReactNode } from "react";
import { useDraft } from "./draft";

/** Device scope key prefix. When `null` (unpaired / same-origin / mock), no
 *  prefix is added — those modes have only one data source, and a prefix would
 *  cause existing user drafts to vanish. */
export function scopedKey(deviceId: string | null, key: string): string {
  return deviceId ? `d/${deviceId}/${key}` : key;
}

const DeviceScopeContext = createContext<string | null>(null);

export function DeviceScopeProvider({
  deviceId,
  children,
}: {
  deviceId: string | null;
  children: ReactNode;
}) {
  return (
    <DeviceScopeContext.Provider value={deviceId}>{children}</DeviceScopeContext.Provider>
  );
}

/** The id of the current scope's device, or `null` if none. */
export function useDeviceScope(): string | null {
  return useContext(DeviceScopeContext);
}

/** Device-scoped version of `useDraft`. Use this for drafts where content is
 *  meaningful only to a specific machine — pure UI preferences (sorting,
 *  collapsing, filter toggles) still use plain `useDraft`, as they belong to
 *  this phone, not to a specific Fleet. */
export function useDeviceDraft<T>(
  key: string,
  fallback: T,
): [T, (v: T | ((prev: T) => T)) => void, () => void] {
  const deviceId = useDeviceScope();
  return useDraft<T>(scopedKey(deviceId, key), fallback);
}
