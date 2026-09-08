import type { PendingDecision, PendingDecisions } from "./types";

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
