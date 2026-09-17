import type { SessionInfo } from "../types";
import { QUIET_ALIVE_COLOR, WATCHING_COLOR, rowBarColor } from "../types";

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
 *  `rowBarColor`'s own green-beats-amber priority.
 *
 *  The faded green (quiet-alive, see [`isQuietAlive`]) is the third rank, and
 *  leaving it out is how a collapsed chain used to read as *ended* while its tip
 *  was very much running: a session parked on one long tool call decays to a
 *  quiet-alive faded dot, this function matched neither literal, and the header
 *  fell through to `null` — no dot at all, while the detail composer for the
 *  same session said "Session Running". Rank by salience rather than by two
 *  hard-coded strings so any colour `rowBarColor` can return survives the
 *  collapse; the phone's `chainTone` (mobile-web `TasksView`) already does
 *  exactly this. */
const BAR_PRIORITY = [
  "var(--color-success)",
  "var(--color-warning)",
  QUIET_ALIVE_COLOR,
  // Last: a hop parked on a watch is the least urgent of the four — nothing is
  // running and nobody is being waited on — but it still beats no dot at all.
  WATCHING_COLOR,
];

export function chainBarColor(members: SessionInfo[]): string | null {
  let best: string | null = null;
  let bestRank = BAR_PRIORITY.length;
  for (const m of members) {
    const c = rowBarColor(m);
    if (c == null) continue;
    const r = BAR_PRIORITY.indexOf(c);
    // An unranked colour still beats no dot at all: a new `rowBarColor` hue
    // must not silently vanish from collapsed rows the way the faded green did.
    const rank = r >= 0 ? r : BAR_PRIORITY.length - 0.5;
    if (rank < bestRank) {
      bestRank = rank;
      best = c;
      if (rank === 0) break; // a running hop wins outright
    }
  }
  return best;
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
