/** Number formatting for the 64px collapsed sidebar rail.
 *
 *  The rail gives a tile ~46px of inner width, so a value only reads if it
 *  stays within ~6 glyphs. Both formatters here trade precision for fit on
 *  purpose — the exact figure always stays available in the tile's tooltip,
 *  and the expanded sidebar shows it in full. */

/** Money for a rail tile: `$6.62` / `$312.40` / `$3.2k` / `$12k`.
 *  Cents are dropped once the整数部分 alone would blow the tile. */
export function fmtRailMoney(usd: number): string {
  const n = Number.isFinite(usd) ? usd : 0;
  const sign = n < 0 ? "-" : "";
  const abs = Math.abs(n);
  if (abs >= 10_000) return `${sign}$${Math.round(abs / 1000)}k`;
  if (abs >= 1_000) return `${sign}$${(abs / 1000).toFixed(1)}k`;
  if (abs >= 100) return `${sign}$${abs.toFixed(0)}`;
  return `${sign}$${abs.toFixed(2)}`;
}

/** Counts for a rail tile: `215` / `1.2k` / `34k`. */
export function fmtRailCount(n: number): string {
  const v = Number.isFinite(n) ? Math.max(0, Math.round(n)) : 0;
  if (v >= 10_000) return `${Math.round(v / 1000)}k`;
  if (v >= 1_000) return `${(v / 1000).toFixed(1)}k`;
  return `${v}`;
}

/** Nav-item alert badge count. Capped at `99+`: the badge is an attention
 *  signal, and an exact four-digit count grows the pill wide enough to cover
 *  the whole icon it is anchored to in the collapsed rail. */
export function fmtBadgeCount(n: number): string {
  const v = Number.isFinite(n) ? Math.max(0, Math.round(n)) : 0;
  return v > 99 ? "99+" : `${v}`;
}
