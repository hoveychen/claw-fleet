/**
 * URL of one decision-asset file through the `fleet-decision` custom protocol.
 *
 * Assets a decision card attached are copied to
 * `~/.fleet/decision-assets/<id>/<qidx>/` and served back from there, so an
 * image-bearing card re-renders exactly as the user saw it.
 *
 * Built by hand (not `convertFileSrc`) for the same reason as WikiView's
 * `wikiFileUrl`: percent-encoding the whole path collapses it to one segment
 * and breaks relative-asset resolution inside the served index.html.
 * `qidx` is the question's `q<step>` dir; `rel` defaults to the served entry.
 *
 * Lives in its own module rather than inside `DecisionPanel`: the inline
 * decision card in the conversation needs it too, and importing it from
 * `DecisionPanel` would close the cycle
 * `MessageList → DecisionToolCard → DecisionPanel → SessionDetail → MessageList`.
 */
export function decisionAssetUrl(id: string, qidx: string, rel = "index.html"): string {
  const path = [id, qidx, ...rel.split("/")].map(encodeURIComponent).join("/");
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-decision.localhost/${path}`
    : `fleet-decision://localhost/${path}`;
}
