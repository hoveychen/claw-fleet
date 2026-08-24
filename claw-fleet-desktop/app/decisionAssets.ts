import { isWebBuild } from "./hostEnv";

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
 * In the browser build there is no custom protocol to reach: the scheme is
 * registered on the Tauri webview, and Chromium blocks the navigation outright
 * for an `<iframe src="fleet-decision://…">` in a tab — measured, and the card
 * still renders its question, options and submit button, so the empty preview
 * rectangle is the only symptom. The same bytes are already served over HTTP by
 * the process that served the page.
 *
 * What it may NOT become there is `/decision_asset?id=…`: the served
 * `index.html` finds the question's images through relative refs
 * (`<img src="chart.png">` is the documented contract for `fleet__ask`'s
 * `images`), and the browser resolves those against the URL's *directory*.
 * `/decision_asset/` keeps every segment in place. No `%2F` smuggling is needed
 * on the way — neither the id nor `q<idx>` may hold a separator.
 *
 * Lives in its own module rather than inside `DecisionPanel`: the inline
 * decision card in the conversation needs it too, and importing it from
 * `DecisionPanel` would close the cycle
 * `MessageList → DecisionToolCard → DecisionPanel → SessionDetail → MessageList`.
 */
export function decisionAssetUrl(id: string, qidx: string, rel = "index.html"): string {
  const path = [id, qidx, ...rel.split("/")].map(encodeURIComponent).join("/");
  if (isWebBuild()) return `${window.location.origin}/decision_asset/${path}`;
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-decision.localhost/${path}`
    : `fleet-decision://localhost/${path}`;
}
