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

/** One round trip attributed to the three places it can be spent. Each is null
 *  when its input was missing — an unknown segment must stay unknown rather
 *  than collapse to 0, or the UI would name the wrong culprit. */
export interface RttSplit {
  totalMs: number;
  /** This phone ↔ relay (its own uplink/downlink). */
  phoneMs: number | null;
  /** Relay ↔ desktop, plus whatever the desktop spends before its handler
   *  starts — decrypt, inflate, JSON parse, task scheduling. Those are not
   *  measured separately today, so they land here rather than in `handleMs`. */
  desktopLinkMs: number | null;
  /** The desktop's handler body (its self-reported `handle_ms`). */
  handleMs: number | null;
}

/** One-line breakdown for the header tooltip: `620ms · 手机 180 · 桌面链路 60 ·
 *  处理 380`. Segments that weren't measured are omitted rather than shown as 0.
 *
 *  Takes the translate function instead of importing it: this module is unit
 *  tested under node, and `i18n` touches `window`/`localStorage` at import. */
export function formatRttSplit(split: RttSplit, t: (s: string) => string): string {
  const parts = [`${split.totalMs}ms`];
  if (split.phoneMs !== null) parts.push(`${t("手机")} ${split.phoneMs}`);
  if (split.desktopLinkMs !== null) parts.push(`${t("桌面链路")} ${split.desktopLinkMs}`);
  if (split.handleMs !== null) parts.push(`${t("处理")} ${split.handleMs}`);
  return parts.join(" · ");
}

/** Attribute a round trip to phone link / desktop link / desktop handler.
 *
 *  The desktop-link leg is a residual: it is what the total has left after the
 *  two directly measured segments, so it needs both of them. Clamped at 0 —
 *  the phone's `Date.now()` can step (NTP, sleep/wake) and the two segments are
 *  measured on different clocks, so a slightly negative residual is a
 *  measurement artefact, not a link that took negative time. */
export function splitRtt(sample: {
  totalMs: number;
  phoneRelayMs: number | null;
  desktopHandleMs: number | null;
}): RttSplit {
  const { totalMs, phoneRelayMs: phoneMs, desktopHandleMs: handleMs } = sample;
  const desktopLinkMs =
    phoneMs !== null && handleMs !== null ? Math.max(0, totalMs - phoneMs - handleMs) : null;
  return { totalMs, phoneMs, desktopLinkMs, handleMs };
}
