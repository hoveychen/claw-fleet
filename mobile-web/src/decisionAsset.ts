// Decision-card asset fetch (images referenced by fleet__ask html / gallery).
// Kept out of DecisionsView so the relay round-trip is unit-testable without a
// React render. The bytes come back base64-framed through the relay's
// `decision_asset` method (see claw-fleet-core/src/mobile_relay.rs).

import { ASSET_REQUEST_TIMEOUT_MS, type FleetTransport } from "./transport";

export interface DecisionAsset {
  mime: string;
  base64: string;
}

/** Fetch one decision-card asset by (request id, question index, bare name).
 *  Uses the generous asset timeout, not the 15s control-message default: asset
 *  bytes are MB-scale over a possibly-slow phone link, and a spurious 15s abort
 *  strands the card's <img> forever (the reply arrives after the pending entry
 *  was already dropped, so it is silently discarded). */
export function fetchDecisionAsset(
  client: FleetTransport,
  requestId: string,
  qidx: number,
  name: string,
): Promise<DecisionAsset> {
  return client.request<DecisionAsset>(
    "decision_asset",
    { id: requestId, qidx: `q${qidx}`, rel: name },
    ASSET_REQUEST_TIMEOUT_MS,
  );
}
