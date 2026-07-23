import type { SessionInfo } from "../types";
import { rowBarColor } from "../types";

/** How many chain members an expanded group shows before "load more"; a relay
 *  chain can run 50 hops deep, so we reveal the most recent few and page in the
 *  rest on demand rather than dumping the whole chain. */
export const GROUP_VISIBLE = 3;
/** Each "load more" click reveals this many more of the earlier hops. */
export const GROUP_LOAD_STEP = 10;

/** Highest-hop (tip / latest relay) member — the chain's "current" session. */
export function chainTip(members: SessionInfo[]): SessionInfo {
  return members.reduce((a, b) => ((b.handoff?.hop ?? 0) > (a.handoff?.hop ?? 0) ? b : a));
}

/** Run-status colour for a *collapsed* relay group's header. A group hides its
 *  other hops, so the header must surface the whole chain's liveness — not just
 *  the tip's. The tip alone is wrong because the group is sorted to the top by
 *  its most recently active member (`agentLastActivityMs`), which need not be
 *  the highest hop: a chain can float up on a mid-chain hop's activity while its
 *  tip sits done, leaving the header with no dot. Aggregate instead, preferring
 *  a running member (green) over a merely waiting one (amber), matching
 *  `rowBarColor`'s own green-beats-amber priority. */
export function chainBarColor(members: SessionInfo[]): string | null {
  let waiting = false;
  for (const m of members) {
    const c = rowBarColor(m);
    if (c === "var(--color-success)") return c; // a running hop wins outright
    if (c === "var(--color-warning)") waiting = true;
  }
  return waiting ? "var(--color-warning)" : null;
}

/** One entry in the rendered task list: either a standalone session or a
 *  collapsed handoff-relay chain. */
export type RenderItem =
  | { kind: "single"; key: string; session: SessionInfo }
  | {
      kind: "group";
      key: string;
      chainId: string;
      /** Total hops in the chain (from `handoff.chainLen`), independent of how
       *  many members are actually present in the filtered list. */
      chainLen: number;
      tip: SessionInfo;
      /** Members present in the (already filtered+sorted) input, newest hop
       *  first. NOT necessarily the whole chain — see `chainMembersAll` for the
       *  full membership used by the group's mark-all action. */
      members: SessionInfo[];
    };

/**
 * Fold a flat, already-filtered+sorted row list into render items, collapsing
 * sessions that share a `handoff.chainId` into one group. A chain becomes a
 * group only when ≥2 of its members are present in `rows`; a lone surviving hop
 * renders as an ordinary row (keeping its own handoff chip) rather than a
 * one-child group. The group takes the list position of its first — i.e. most
 * recently active, since `rows` arrives sorted by activity — member; members are
 * ordered newest-hop-first so "show last N" reveals the recent relays first.
 */
export function buildRenderItems(rows: SessionInfo[], group: boolean): RenderItem[] {
  if (!group) return rows.map((s) => ({ kind: "single", key: s.jsonlPath, session: s }));
  const items: RenderItem[] = [];
  const groupAt = new Map<string, number>();
  for (const s of rows) {
    const cid = s.handoff && s.handoff.chainLen > 1 ? s.handoff.chainId : null;
    if (!cid) {
      items.push({ kind: "single", key: s.jsonlPath, session: s });
      continue;
    }
    const at = groupAt.get(cid);
    if (at === undefined) {
      groupAt.set(cid, items.length);
      items.push({
        kind: "group",
        key: `chain:${cid}`,
        chainId: cid,
        chainLen: s.handoff!.chainLen,
        tip: s,
        members: [s],
      });
    } else {
      (items[at] as Extract<RenderItem, { kind: "group" }>).members.push(s);
    }
  }
  return items.map((it) => {
    if (it.kind !== "group") return it;
    if (it.members.length < 2) {
      return { kind: "single", key: it.members[0].jsonlPath, session: it.members[0] };
    }
    const members = [...it.members].sort((a, b) => b.handoff!.hop - a.handoff!.hop);
    return { ...it, tip: chainTip(members), members };
  });
}

/**
 * Which sessions a dwell-read on the active session should clear. A collapsed
 * relay-group header shows an aggregate unread dot over its *whole* chain (see
 * the header's `unread` prop, which is `full.some(sessionUnread)`), yet clicking
 * the header only opens the tip. So dwelling on it must mark every chain member
 * read — otherwise a still-unread non-tip hop keeps the group's dot lit after
 * the user has plainly opened it. A standalone row, an expanded group's child,
 * or any ungrouped row marks only itself.
 *
 * `groupTips` maps a currently-rendered group header's tip id → that group's
 * full chain membership; ids absent from it (singles / children) fall back to
 * the lone session.
 */
export function dwellReadTargets(
  active: SessionInfo,
  groupTips: Map<string, SessionInfo[]>,
): SessionInfo[] {
  const chain = groupTips.get(active.id);
  return chain && chain.length > 0 ? chain : [active];
}
