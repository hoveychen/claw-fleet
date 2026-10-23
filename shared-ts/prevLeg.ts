/**
 * "Load the previous relay leg" — the pure half shared by the desktop
 * SessionDetail and the mobile SessionDetailView.
 *
 * A relay chain is a list of handoff links (`from → to`). The leg open in the
 * detail pane can prepend its predecessor's transcript above its own, then
 * that leg's predecessor, and so on back to leg 1. Each client owns the
 * fetching and the rendering; this module owns the two decisions both must
 * make identically: which link names the predecessor, and how the stitched
 * message list is laid out (with one synthetic heading row opening each leg).
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

/** True once a later leg has joined the chain behind this session, i.e. it
 *  handed off and its successor is running. The backend recomputes `chainLen`
 *  on every scan, so an open pane learns this without re-reading anything. */
export function hasSuccessor(handoff: { hop: number; chainLen: number } | null | undefined): boolean {
  return !!handoff && handoff.hop < handoff.chainLen;
}

/** The leg that took over from `sessionId`, for the "go to the next leg"
 *  control. The title falls back to the id while the successor is not yet in
 *  this client's scan. Null for the chain's last leg. */
export function successorLeg(args: {
  links: readonly LegLinkShape[];
  sessionId: string;
  hop: number;
  sessions: readonly { id: string; titleOverride?: string | null; aiTitle?: string | null }[];
}): { sessionId: string; hop: number; title: string } | null {
  const link = args.links.find((l) => l.fromSessionId === args.sessionId);
  if (!link) return null;
  const s = args.sessions.find((x) => x.id === link.toSessionId);
  return {
    sessionId: link.toSessionId,
    hop: args.hop + 1,
    title: legTitle(s ?? { id: link.toSessionId }),
  };
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

/** A leg's display title: the chosen title, the inferred one, else the id. */
export function legTitle(s: { id: string; titleOverride?: string | null; aiTitle?: string | null }): string {
  return s.titleOverride || s.aiTitle || `${s.id.slice(0, 8)}…`;
}

/** What a leg heading says about the leg directly below it. */
export interface LegHeadingInfo {
  /** 1-based hop of the leg below the heading. */
  hop: number;
  sessionId: string;
  title: string;
  /** Epoch ms that leg started (see `legStartedAt`). */
  startedAt: number;
  /** The leg open in the pane, as opposed to a prepended one. */
  current: boolean;
}

/** One prepended leg, as fetched so far (tail-first, like the current leg). */
export interface LoadedLeg<M> {
  hop: number;
  sessionId: string;
  title: string;
  /** The session's `createdAtMs` (its transcript's creation time): the start
   *  time until the leg is paged back to its first message. */
  createdAtMs: number;
  /** The leg is paged back to its first message. */
  fullyLoaded: boolean;
  messages: M[];
}

/** Epoch ms of the first record carrying a timestamp, or null. */
export function firstTimestamp(messages: readonly unknown[]): number | null {
  for (const m of messages) {
    const ts = (m as { timestamp?: unknown } | null)?.timestamp;
    if (typeof ts !== "string") continue;
    const ms = Date.parse(ts);
    if (!Number.isNaN(ms)) return ms;
  }
  return null;
}

/** When a leg started: its first message's timestamp once the leg is loaded
 *  back to that message, the session's creation time before that. */
export function legStartedAt(leg: { messages: readonly unknown[]; fullyLoaded: boolean; createdAtMs: number }): number {
  return (leg.fullyLoaded ? firstTimestamp(leg.messages) : null) ?? leg.createdAtMs;
}

/** The heading of a leg, prepended or current. */
export function legHeading(
  leg: { hop: number; sessionId: string; title: string; messages: readonly unknown[]; fullyLoaded: boolean; createdAtMs: number },
  current: boolean,
): LegHeadingInfo {
  return { hop: leg.hop, sessionId: leg.sessionId, title: leg.title, startedAt: legStartedAt(leg), current };
}

const HEADING_UUID_PREFIX = "fleet-leg-heading:";

/** The synthetic row opening a leg in a stitched list. Typed as a user record
 *  so every row predicate already in the clients treats it as a full row: it
 *  ends the previous leg's last turn (usage line), breaks a work run, and is
 *  never folded into a meta group. Renderers detect it by `_fleetLegHeading`
 *  and draw the heading instead of a bubble. */
export interface LegHeadingRow {
  type: "user";
  uuid: string;
  timestamp: string;
  message: { role: "user"; content: { type: "text"; text: string }[] };
  _fleetLegHeading: LegHeadingInfo;
}

export function legHeadingRow(info: LegHeadingInfo): LegHeadingRow {
  return {
    type: "user",
    uuid: `${HEADING_UUID_PREFIX}${info.sessionId}`,
    timestamp: new Date(info.startedAt).toISOString(),
    message: { role: "user", content: [{ type: "text", text: "" }] },
    _fleetLegHeading: info,
  };
}

/** The heading payload of a row, or null when it is a real record. */
export function legHeadingOf(msg: unknown): LegHeadingInfo | null {
  const d = (msg as { _fleetLegHeading?: LegHeadingInfo } | null)?._fleetLegHeading;
  return d ?? null;
}

/** The current leg as the stitcher needs it. */
export interface CurrentLegInfo<M> {
  hop: number;
  sessionId: string;
  title: string;
  createdAtMs: number;
  fullyLoaded: boolean;
  messages: M[];
}

/**
 * Lay out `legs` (nearest predecessor first, as they were loaded) above the
 * current leg's messages, oldest leg at the top, each opened by its heading.
 * Once anything is prepended the current leg gets a heading too, marked as
 * current. Returns `current.messages` itself when nothing is prepended, so
 * callers memoizing on identity keep their cache.
 */
export function stitchLegs<M>(
  legs: readonly LoadedLeg<M>[],
  current: CurrentLegInfo<M>,
): (M | LegHeadingRow)[] {
  if (legs.length === 0) return current.messages;
  const out: (M | LegHeadingRow)[] = [];
  for (let i = legs.length - 1; i >= 0; i--) {
    out.push(legHeadingRow(legHeading(legs[i], false)), ...legs[i].messages);
  }
  out.push(legHeadingRow(legHeading(current, true)), ...current.messages);
  return out;
}

/** How many leading entries of a stitched list are not the current leg's own
 *  records: the prepended legs' messages and every heading, the current
 *  leg's included. */
export function prependedCount<M>(legs: readonly LoadedLeg<M>[]): number {
  if (legs.length === 0) return 0;
  let n = 1;
  for (const l of legs) n += l.messages.length + 1;
  return n;
}
