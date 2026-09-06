/**
 * React identity for the transcript detail surface.
 *
 * The same session id can exist on two paired desktops, so both parts are
 * required. A changed key remounts SessionDetailView and prevents its previous
 * transcript from remaining visible while the next session bootstraps.
 */
export function sessionDetailKey(deviceId: string, sessionId: string): string {
  return `${deviceId}\u0000${sessionId}`;
}
