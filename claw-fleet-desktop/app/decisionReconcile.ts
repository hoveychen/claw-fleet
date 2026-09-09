import type { PendingDecision, PendingDecisions } from "./types";

/**
 * How long a card this client removed is kept from coming back.
 *
 * The reconcile poll and the answer round trip race, and the reconcile is the
 * faster of the two. Answering removes the card immediately and posts the
 * response in the background (see `fireDecisionResponse`) — the request file
 * stays on disk until the blocked `fleet mcp` picks the response up on its own
 * 200ms poll and cleans up. A reconcile landing in that gap sees the card still
 * pending and would put it straight back on screen, answered.
 *
 * So a removal is remembered for a while, and only for a while: if the card is
 * *still* pending after the grace, that is no longer a race — the answer was
 * genuinely lost (a dropped write, a wedged producer) and the honest thing is to
 * show the question again rather than leave an agent blocked on a card the user
 * can no longer see. 30s matches the phone's `ANSWER_GRACE_MS`, which balances
 * the same two failure modes.
 */
export const REMOVED_GRACE_MS = 30_000;

/** id → when this client removed it. Pruned on read. */
const removedLocallyAt = new Map<string, number>();

/**
 * Remember that this client removed a card — answered, declined, dismissed, or
 * pruned. Called from the store's single removal funnel, so every path is
 * covered without each one having to remember to.
 */
export function noteRemovedLocally(id: string, now: number = Date.now()): void {
  removedLocallyAt.set(id, now);
}

/** Ids the reconcile must not re-add yet. Prunes entries past the grace. */
export function suppressedIds(now: number = Date.now()): Set<string> {
  for (const [id, at] of removedLocallyAt) {
    if (now - at > REMOVED_GRACE_MS) removedLocallyAt.delete(id);
  }
  return new Set(removedLocallyAt.keys());
}

/** Test seam — module state would otherwise leak between cases. */
export function clearRemovedLocally(): void {
  removedLocallyAt.clear();
}

/** Every request in a pending snapshot, flattened across the six channels. */
export function flattenPending(p: PendingDecisions): Map<string, { parked?: boolean }> {
  const out = new Map<string, { parked?: boolean }>();
  for (const bucket of [
    p.guard,
    p.elicitation,
    p.fleetAsk,
    p.a2uiRender,
    p.planApproval,
    p.permissionPrompt,
  ]) {
    bucket?.forEach((r) => out.set(r.id, r as { parked?: boolean }));
  }
  return out;
}

/**
 * What the reconcile poll should do about one card it found pending.
 *
 * `announce` chimes and speaks; `record` marks the card as already-known
 * *without* a sound; `skip` leaves the bookkeeping alone.
 */
export type ReconcileAnnouncement = "announce" | "record" | "skip";

/**
 * Whether a pending card the poll just saw is news worth a chime.
 *
 * The `record` case is the whole reason this is a function. The mount poll is
 * deliberately silent — a card that predates the page is not news — but it used
 * to be silent *and* forgetful: it never wrote the id into the announced set,
 * so the very next tick, 10 seconds later, saw the same card as unseen and
 * chimed it. Every page load, every reconnect that remounted the panel, and
 * every card still sitting on the backend re-announced itself: Boss heard a
 * chime, went looking, and found nothing new — the card had been there for half
 * an hour. Recording on mount keeps the silence and the memory together.
 *
 * A parked card is neither announced nor recorded: it is an old question being
 * re-listed, it never chimes on any path, and leaving it out of the set matches
 * what the live listeners do.
 */
export function announcementFor(
  why: string,
  parked: boolean | undefined,
  alreadyAnnounced: boolean,
): ReconcileAnnouncement {
  if (alreadyAnnounced || parked) return "skip";
  return why === "mount" ? "record" : "announce";
}

/** What the store has to change to agree with a pending snapshot. */
export interface ReconcilePlan {
  /** Cards to remove: the backend no longer has them pending. */
  drop: string[];
  /** Cards to flip into the parked state in place. */
  park: string[];
}

/**
 * Diff the decision store against the backend's pending set.
 *
 * The store is fed by one-shot emits (`*-request`, `*-dismissed`,
 * `decision-parked`), none of which are replayed to a listener that was not
 * attached at emit time. Both directions of a lost emit are user-visible: a
 * missed request is a card nobody ever sees, and a missed dismissal is a card
 * that answers with "no pending request". So the panel re-derives its set from
 * the backend on a timer, and this is that derivation.
 *
 * Adding is left to the caller's `add*` actions, which already dedup by id.
 * What needs care is the removal side:
 *
 *   - Only ids in `idsBefore` — the store's contents *before* the snapshot was
 *     requested — are eligible to be dropped. A card that arrived while the
 *     request was in flight is legitimately absent from the answer, because the
 *     backend read its directory before that card existed. Without this guard
 *     the reconcile races the live channel and deletes brand-new cards.
 *   - The caller must not run this at all on a failed fetch: "the request
 *     errored" and "nothing is pending" both look like an empty snapshot, and
 *     acting on the first would clear the panel whenever the network blinks.
 */
export function reconcilePlan(
  decisions: PendingDecision[],
  idsBefore: Set<string>,
  pending: Map<string, { parked?: boolean }>,
): ReconcilePlan {
  const plan: ReconcilePlan = { drop: [], park: [] };
  for (const d of decisions) {
    const live = pending.get(d.id);
    if (!live) {
      if (idsBefore.has(d.id)) plan.drop.push(d.id);
      continue;
    }
    if (live.parked && !(d.request as { parked?: boolean }).parked) plan.park.push(d.id);
  }
  return plan;
}
