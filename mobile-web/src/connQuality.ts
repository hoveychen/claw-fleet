// Weak-link congestion classification for the mobile connection indicator.
// Two independent signals — request→reply RTT and reconnect frequency — are each
// mapped to a 3-level rank, then combined by taking the worse of the two. Kept
// as pure functions so the header signal light is unit-testable in isolation.

export type Congestion = "good" | "fair" | "congested";

const RANK: Record<Congestion, number> = { good: 0, fair: 1, congested: 2 };

/** RTT thresholds (ms). A healthy link answers control-message round-trips well
 *  under 300ms; past ~1.2s the UI is visibly laggy. */
export function rttLevel(rttMs: number | null): Congestion {
  if (rttMs === null) return "good";
  if (rttMs > 1200) return "congested";
  if (rttMs > 300) return "fair";
  return "good";
}

/** Reconnects observed within the recent window. A flaky link reconnects
 *  repeatedly; a single blip is "fair", three or more is "congested". */
export function reconnectLevel(recentReconnects: number): Congestion {
  if (recentReconnects >= 3) return "congested";
  if (recentReconnects >= 1) return "fair";
  return "good";
}

/** The higher-severity of two levels. */
export function worse(a: Congestion, b: Congestion): Congestion {
  return RANK[a] >= RANK[b] ? a : b;
}

/** Combine both weak-link signals into one level (the worse of the two). */
export function computeCongestion(
  rttMs: number | null,
  recentReconnects: number,
): Congestion {
  return worse(rttLevel(rttMs), reconnectLevel(recentReconnects));
}

/** Window over which reconnects count toward congestion. */
export const RECONNECT_WINDOW_MS = 60_000;
