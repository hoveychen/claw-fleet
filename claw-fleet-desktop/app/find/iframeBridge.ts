/**
 * Parent half of the find bridge to sandboxed preview iframes.
 *
 * The previews (decision-card `html`, served wiki HTML) render in
 * `sandbox="allow-scripts"` iframes with no `allow-same-origin`, so the parent
 * cannot reach into `contentDocument` to search them. Instead each document
 * carries the in-iframe find handler (`mcp_ipc::FIND_SCRIPT`); we drive it over
 * `postMessage` and it reports its match count back. These helpers are the thin
 * message layer — the ordering/navigation logic lives in useFindController.
 */

/** A search/navigate command sent down to an iframe's find handler. */
type FindCommand =
  | { action: "search"; q: string }
  | { action: "goto"; index: number }
  | { action: "clear" };

/** A match-count reply parsed from an iframe. */
export interface FindResult {
  count: number;
}

function post(frame: HTMLIFrameElement, cmd: FindCommand) {
  frame.contentWindow?.postMessage({ __fleetFind: cmd }, "*");
}

/** Every iframe currently in the document that could host the find handler. */
export function listSearchableFrames(): HTMLIFrameElement[] {
  return Array.from(document.querySelectorAll("iframe")).filter((f) => !!f.contentWindow);
}

export function frameSearch(frame: HTMLIFrameElement, q: string) {
  post(frame, { action: "search", q });
}

/** Mark match `index` active inside the frame (`-1` clears the active mark). */
export function frameGoto(frame: HTMLIFrameElement, index: number) {
  post(frame, { action: "goto", index });
}

export function frameClear(frame: HTMLIFrameElement) {
  post(frame, { action: "clear" });
}

/**
 * Parse a `__fleetFindResult` payload, or return null if the message isn't one.
 * The payload crosses a trust boundary (agent-authored, opaque origin), so every
 * field is validated before use.
 */
export function parseFindResult(data: unknown): FindResult | null {
  if (!data || typeof data !== "object") return null;
  const inner = (data as { __fleetFindResult?: unknown }).__fleetFindResult;
  if (!inner || typeof inner !== "object") return null;
  const count = (inner as { count?: unknown }).count;
  if (typeof count !== "number" || !Number.isFinite(count) || count < 0) return null;
  return { count: Math.floor(count) };
}
