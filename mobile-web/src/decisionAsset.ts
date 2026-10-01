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

/**
 * `<img src>` attributes in an agent-authored preview, in all three HTML
 * attribute syntaxes: double-quoted, single-quoted and unquoted. Agents write
 * `<img src=chart.png>` as often as the quoted form; matching only the quoted
 * ones left every unquoted ref as a broken-image glyph inside the sandboxed
 * frame (the relative ref has nothing to resolve against in a `srcDoc`).
 * Mirrors `IMG_SRC_RE` in claw-fleet-desktop/app/decisionAssetDoc.ts.
 */
const IMG_SRC_RE = /(<img\b[^>]*?\bsrc\s*=\s*)(?:(["'])([^"']*)\2|([^\s"'=<>`]+))/gi;

/** Rewrite every `<img src>` whose value is a key of `uris` to that URI,
 *  keeping the original quoting (unquoted refs come back double-quoted, since a
 *  `data:` URI may hold characters an unquoted value cannot). */
export function inlineImgSrcs(html: string, uris: ReadonlyMap<string, string>): string {
  if (uris.size === 0) return html;
  return html.replace(
    IMG_SRC_RE,
    (whole, pre: string, quote: string | undefined, quoted: string | undefined, bare: string | undefined) => {
      const uri = uris.get(quoted ?? bare ?? "");
      if (!uri) return whole;
      const q = quote ?? '"';
      return `${pre}${q}${uri}${q}`;
    },
  );
}
