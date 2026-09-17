/**
 * How many *units of work* a flat session list represents — one per standalone
 * session, one per handoff-relay chain no matter how many hops it carries.
 *
 * The task page's counters used to count raw sessions, which answers a question
 * nobody asks: a 12-hop relay chain is one task being worked, not twelve. With
 * most work now running as chains, "全部 330 / 22 / 308" stopped being readable
 * as "how many things am I running" — the number tracked how often sessions had
 * handed off, not how much was in flight.
 *
 * Deliberately *not* implemented as `buildRenderItems(rows).length`: that
 * function's "a chain needs ≥2 present members to collapse" rule exists so a
 * lone surviving hop renders as an ordinary row rather than a one-child group,
 * which is a rendering concern. For counting, a chain with one visible hop is
 * still one unit either way, so the rule cancels out and a set of keys is both
 * cheaper and clearer.
 *
 * `chainKeyOf` returns the chain's identity — namespaced by whatever scope the
 * caller's list groups within (repository root on the desktop, device + section
 * on the phone), so two machines that coincidentally share a `chainId` are not
 * folded into one unit. Returning `null` means "this row is its own unit",
 * which is also how callers express the handoff-grouping setting being off.
 */
export function countChainUnits<T>(rows: T[], chainKeyOf: (row: T) => string | null): number {
  let singles = 0;
  const chains = new Set<string>();
  for (const row of rows) {
    const key = chainKeyOf(row);
    if (key == null) singles += 1;
    else chains.add(key);
  }
  return singles + chains.size;
}
