// Wait for a freshly-opened transport to be ready.
//
// After multi-device support, one task must happen on **devices other than the current one**: when removing a device,
// tell its relay channel to stop pushing. That device has no live connection, so open a temporary one, send one frame, then close.
// "Are we connected?" has only one honest answer on FleetTransport: `isAuthed` (it's always true for same-origin HTTP,
// for relay it means handshake complete), so we poll it here — the event-callback path requires callers to wire handlers
// at construction time, but temporary connections have no interest in those events anyway.

import type { FleetTransport } from "./transport";

/** Timeout budget for temporary connections waiting for handshake. Relay handshakes typically complete in hundreds of ms;
 *  5s covers weak networks, beyond which we should honestly tell the user "unsubscribe failed, try next time" instead of making them wait. */
export const AUTH_WAIT_MS = 5_000;

/** Poll interval. Dense enough to avoid idle waiting on fast links, light enough not to starve the main thread. */
const POLL_MS = 100;

/** Wait until `isAuthed` is true or the timeout is reached. Returns whether it was reached. */
export function waitAuthed(
  client: FleetTransport,
  budgetMs: number = AUTH_WAIT_MS,
  pollMs: number = POLL_MS,
): Promise<boolean> {
  if (client.isAuthed) return Promise.resolve(true);
  return new Promise((resolve) => {
    const deadline = Date.now() + budgetMs;
    const timer = window.setInterval(() => {
      if (client.isAuthed) {
        window.clearInterval(timer);
        resolve(true);
      } else if (Date.now() >= deadline) {
        window.clearInterval(timer);
        resolve(false);
      }
    }, pollMs);
  });
}
