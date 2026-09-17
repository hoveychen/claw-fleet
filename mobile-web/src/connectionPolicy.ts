// Connection policy for multiple devices: when to connect, how often to poll.
//
// With a single device, these constants can be anything. With N devices,
// they should be multiplied by N: three devices means three heartbeats,
// three daily-spend polls, three reconciliation polls. In weak network or
// low-power scenarios, this is not "slightly slower" — it fully saturates
// what is already a narrow link — and most of those requests ask about
// devices the user is not looking at.
//
// The policy is extracted as pure functions here (rather than scattered
// across multiple effects) because it encodes a set of meaningful trade-offs
// worth pinning down with unit tests:
//
//   * **No background connections.** Disconnect all sockets after the page
//     is hidden long enough. The background channel is push-based anyway
//     (subscriptions live on the relay, independent of socket existence),
//     so continuing to hold N connections just burns battery. Give a grace
//     period to avoid re-handshaking N times when switching back.
//   * **Non-active devices poll slower.** Daily spend is that top number,
//     merged across all devices; but the device the user is actively viewing
//     changes most relevantly, and slower polling of the rest is imperceptible.
//   * **Stagger connections.** N connections reconnecting simultaneously at
//     network recovery causes a burst; stagger by device index by a few
//     hundred milliseconds, with imperceptible latency cost.

/** Disconnect all connections after the page is hidden for this long.
 *
 *  Short interval causes "step away to reply then come back" to re-handshake
 *  every time; long interval leaves N sockets idle after truly leaving.
 *  30 seconds covers most "glance and return" scenarios. */
export const HIDDEN_DISCONNECT_MS = 30_000;

/** Daily spend poll interval for the current active device. */
export const USAGE_POLL_ACTIVE_MS = 20_000;
/** Daily spend poll interval for other devices. Their numbers only feed the
 *  top-line sum; slower polling is imperceptible. */
export const USAGE_POLL_BACKGROUND_MS = 60_000;

/** Stagger delay before each device connects. */
export const CONNECT_STAGGER_MS = 250;
/** Cap on stagger delay: even many devices should not make the last one wait too long. */
export const CONNECT_STAGGER_MAX_MS = 1_500;

export interface VisibilityState {
  visible: boolean;
  /** Timestamp when entering hidden state; undefined when `visible` is true. */
  hiddenSince: number;
}

/** Whether this connection should be kept open now.
 *
 *  Remains open while hidden duration is **below** the grace period — that is
 *  usually just glancing away briefly. */
export function shouldConnect(vis: VisibilityState, now: number): boolean {
  if (vis.visible) return true;
  return now - vis.hiddenSince < HIDDEN_DISCONNECT_MS;
}

/** Daily spend poll interval for this device. */
export function usagePollMs(isActive: boolean): number {
  return isActive ? USAGE_POLL_ACTIVE_MS : USAGE_POLL_BACKGROUND_MS;
}

/** Stagger delay for device at `index`. */
export function connectDelayMs(index: number): number {
  return Math.min(index * CONNECT_STAGGER_MS, CONNECT_STAGGER_MAX_MS);
}
