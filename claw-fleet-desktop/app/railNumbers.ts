/** Number formatting for the nav sidebar. */

/** Nav-item alert badge count. Capped at `99+`: the badge is an attention
 *  signal, and an exact four-digit count grows the pill wide enough to cover
 *  the whole icon it is anchored to. */
export function fmtBadgeCount(n: number): string {
  const v = Number.isFinite(n) ? Math.max(0, Math.round(n)) : 0;
  return v > 99 ? "99+" : `${v}`;
}
