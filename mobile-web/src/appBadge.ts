// Keep the launcher icon badge in step with the number of decision cards
// waiting for an answer.
//
// On HarmonyOS a push can paint the badge itself (Push Kit's
// `notification.badge.setNum`, stamped by the relay), but nothing takes it
// away: opening the app, tapping the notification and clearing it all leave
// the number where it was. So the push is only ever the "there is something
// new" half — this is the half that says how much is left, and the page is the
// only side that can know, because it sees every paired desktop while a push
// speaks only for the one that sent it.
//
// A browser/PWA has no bridge and this is a no-op. `navigator.setAppBadge`
// would cover that case, but it is a separate surface with its own
// permission story, so it is deliberately not wired here.

/** Name of the native bridge object injected by the shell
 *  (mobile-harmony's WebShell.ets: javaScriptProxy). */
const BRIDGE = "fleetNative";

interface NativeBridge {
  setBadge?: (count: number) => void;
}

function bridge(): NativeBridge | undefined {
  return (window as unknown as Record<string, NativeBridge | undefined>)[BRIDGE];
}

/** Whether this shell can paint the launcher badge. False in browsers/PWAs, and
 *  false on an older shell that predates the method — the page then simply
 *  leaves the badge to the push. */
export function canSetAppBadge(): boolean {
  return typeof bridge()?.setBadge === "function";
}

/** Set the badge to `count` (0 clears it). Negative and fractional inputs are
 *  normalised here so no caller has to think about it. */
export function setAppBadge(count: number): void {
  const n = Number.isFinite(count) ? Math.max(0, Math.floor(count)) : 0;
  bridge()?.setBadge?.(n);
}
