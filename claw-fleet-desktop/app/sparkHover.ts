/** Resolve the datum a recharts chart is currently hovering.
 *
 *  recharts hands `activeTooltipIndex` back as `number | string | undefined`
 *  (string only for two-dimensional charts, which sparklines are not), and it
 *  can point past the end of `data` for a tick that no longer exists after the
 *  rolling window trimmed it. Both cases collapse to "nothing hovered". */
export function pickHoverPoint<T>(
  data: readonly T[],
  activeIndex: number | string | null | undefined,
): T | null {
  if (activeIndex == null) return null; // Number(null) is 0 — must not fall through
  const i = typeof activeIndex === "number" ? activeIndex : Number(activeIndex);
  if (!Number.isInteger(i) || i < 0 || i >= data.length) return null;
  return data[i] ?? null;
}
