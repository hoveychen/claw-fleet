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
// A browser/PWA has no bridge, and falls back to the Badging API. That one
// only paints anything for an installed app (a plain tab either rejects or is
// silently ignored, depending on the browser), so the rejection is swallowed
// rather than reported — there is nothing the user could do about it and
// nothing broken if it does nothing.

/** Name of the native bridge object injected by the shell
 *  (mobile-harmony's WebShell.ets: javaScriptProxy). */
const BRIDGE = "fleetNative";

interface NativeBridge {
  setBadge?: (count: number) => void;
}

function bridge(): NativeBridge | undefined {
  return (window as unknown as Record<string, NativeBridge | undefined>)[BRIDGE];
}

/** The Badging API, as much of it as we use. Absent on iOS Safari before 16.4
 *  and on every desktop Firefox, hence the optional methods. */
interface Badging {
  setAppBadge?: (count?: number) => Promise<void>;
  clearAppBadge?: () => Promise<void>;
}

function badging(): Badging {
  return navigator as unknown as Badging;
}

/** Whether anything here can paint a badge: the shell bridge, or the Badging
 *  API. Note that a `true` from the Badging branch only means the method
 *  exists — whether the badge is actually visible depends on the app being
 *  installed, which no API reports. */
export function canSetAppBadge(): boolean {
  return (
    typeof bridge()?.setBadge === "function" || typeof badging().setAppBadge === "function"
  );
}

/** Set the badge to `count` (0 clears it). Negative and fractional inputs are
 *  normalised here so no caller has to think about it. */
export function setAppBadge(count: number): void {
  const n = Number.isFinite(count) ? Math.max(0, Math.floor(count)) : 0;
  const shell = bridge();
  if (typeof shell?.setBadge === "function") {
    shell.setBadge(n);
    return;
  }
  // `setAppBadge(0)` shows a dot in some browsers rather than clearing, so zero
  // goes through the explicit clear.
  const nav = badging();
  const done = n === 0 ? nav.clearAppBadge?.() : nav.setAppBadge?.(n);
  void done?.catch(() => {});
}
