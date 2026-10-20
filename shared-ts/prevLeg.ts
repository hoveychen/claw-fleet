/**
 * "Load the previous relay leg" — the pure half shared by the desktop
 * SessionDetail and the mobile SessionDetailView.
 *
 * A relay chain is a list of handoff links (`from → to`). The leg open in the
 * detail pane can prepend its predecessor's transcript above its own, then
 * that leg's predecessor, and so on back to leg 1. Each client owns the
 * fetching and the rendering; this module owns the two decisions both must
 * make identically: which link names the predecessor, and how the stitched
 * message list is laid out (with one synthetic divider row at each boundary).
 *
 * Prepended legs are read-only display. The stitched list is only ever handed
 * to the renderer — the composer, resume, live-follow polling and optimistic
 * echo all keep reading the current leg's own messages.
 */

/** The fields of a handoff link this module reads. */
export interface LegLinkShape {
  fromSessionId: string;
  toSessionId: string;
  /** Epoch milliseconds the baton passed. */
  handedAt: number;
}

/** The link whose successor is `sessionId`, i.e. the one naming its
 *  predecessor. Null for leg 1, or a session the chain doesn't contain. */
export function predecessorLink<L extends LegLinkShape>(
  links: readonly L[],
  sessionId: string,
): L | null {
  return links.find((l) => l.toSessionId === sessionId) ?? null;
}

/** The leg a "load leg N" control would prepend next, or null when none is
 *  on offer: the topmost loaded leg (the current one when nothing is
 *  prepended) must reach its first message, the chain must name its
 *  predecessor, and that session must be in this client's scan — its
 *  transcript path comes from there. */
export function nextLeg<S extends { id: string }, L extends LegLinkShape>(args: {
  links: readonly L[] | null;
  topId: string;
  topHop: number;
  topFullyLoaded: boolean;
  sessions: readonly S[];
}): { link: L; pred: S; hop: number } | null {
  const { links, topId, topHop, topFullyLoaded, sessions } = args;
  if (!links || !topFullyLoaded || topHop <= 1) return null;
  const link = predecessorLink(links, topId);
  if (!link) return null;
  const pred = sessions.find((s) => s.id === link.fromSessionId);
  return pred ? { link, pred, hop: topHop - 1 } : null;
}

/** What a divider row says about the leg directly above it. */
export interface LegDividerInfo {
  /** 1-based hop of the leg above the divider. */
  hop: number;
  sessionId: string;
  title: string;
  /** Epoch ms that leg handed off to the one below the divider. */
  handedAt: number;
}

/** One prepended leg, as fetched so far (tail-first, like the current leg). */
export interface LoadedLeg<M> extends LegDividerInfo {
  messages: M[];
}

const DIVIDER_UUID_PREFIX = "fleet-leg-divider:";

/** The synthetic row standing at the boundary below a prepended leg. Typed
 *  as a user record so every row predicate already in the clients treats it
 *  as a full row: it ends the leg's last turn (usage line), breaks a work
 *  run, and is never folded into a meta group. Renderers detect it by
 *  `_fleetLegDivider` and draw the divider instead of a bubble. */
export interface LegDividerRow {
  type: "user";
  uuid: string;
  timestamp: string;
  message: { role: "user"; content: { type: "text"; text: string }[] };
  _fleetLegDivider: LegDividerInfo;
}

export function legDividerRow(info: LegDividerInfo): LegDividerRow {
  return {
    type: "user",
    uuid: `${DIVIDER_UUID_PREFIX}${info.sessionId}`,
    timestamp: new Date(info.handedAt).toISOString(),
    message: { role: "user", content: [{ type: "text", text: "" }] },
    _fleetLegDivider: info,
  };
}

/** The divider payload of a row, or null when it is a real record. */
export function legDividerOf(msg: unknown): LegDividerInfo | null {
  const d = (msg as { _fleetLegDivider?: LegDividerInfo } | null)?._fleetLegDivider;
  return d ?? null;
}

/**
 * Lay out `legs` (nearest predecessor first, as they were loaded) above the
 * current leg's messages: oldest leg at the top, each followed by its divider.
 * Returns `current` itself when nothing is prepended, so callers memoizing on
 * identity keep their cache.
 */
export function stitchLegs<M>(legs: readonly LoadedLeg<M>[], current: M[]): (M | LegDividerRow)[] {
  if (legs.length === 0) return current;
  const out: (M | LegDividerRow)[] = [];
  for (let i = legs.length - 1; i >= 0; i--) {
    const { messages, ...info } = legs[i];
    out.push(...messages, legDividerRow(info));
  }
  out.push(...current);
  return out;
}

/** How many leading entries of a stitched list belong to prepended legs
 *  (their messages plus their dividers). */
export function prependedCount<M>(legs: readonly LoadedLeg<M>[]): number {
  let n = 0;
  for (const l of legs) n += l.messages.length + 1;
  return n;
}
