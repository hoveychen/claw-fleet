// The only seam between mobile UI and the "backend".
//
// All data in this app flows through a single object: the view acquires it, calls `request()`,
// and subscribes to callbacks it pushes. Until now, that object could only be `RelayClient`,
// so "via relay" became a synonym for "has data". But they are actually two separate things:
// relay solves "the phone is not on the same network as the desktop", while same-origin
// deployment (`fleet webui` emits both the mobile UI and data routes from the same port)
// has no such problem — no pairing keys, no WebSocket, no relay at all.
//
// So the seam manifests here: `FleetTransport` is the small surface the view actually depends
// on, and `RelayClient` is just its first implementation. The second implementation uses
// same-origin HTTP, and imports no relay code outside this file — this is a hard constraint,
// not a style preference: `relay.ts` runs `resolveRelayBase()` at module load time to resolve
// a relay address, and any browser build that touches it will ship a relay client it never uses.
//
// Therefore, anything not directly tied to "how to send bytes" — error classification, timeout
// budgets for slow methods — lives here, not in any one implementation. `relay.ts` re-exports
// them, and old import paths stay valid.

import type { DecisionKind, SessionInfo } from "./types";

/** A failed `request()`, tagged with "which layer did the failure occur at".
 *
 *  `remote: true` — the host received the request, made a decision, and declined it
 *  (`ok:false` reply). The message is the host's own text. Retrying or waiting won't
 *  change the outcome; show it to the user directly.
 *
 *  `remote: false` — the request never reached a decision: timeout, connection lost, reply
 *  frame dropped. The host may well have already done the work, so the caller has the right
 *  to verify once more by another means before declaring failure (see `waitForSessionId`).
 *
 *  This distinction applies equally to both transport layers, so it belongs in the interface,
 *  not in any one implementation: HTTP's `ok:false` and relay's `reply{ok:false}` are the
 *  same thing, and fetch errors and WebSocket disconnects are also the same thing. */
export class TransportError extends Error {
  constructor(
    message: string,
    readonly remote: boolean,
  ) {
    super(message);
    // Preserve the existing value: historically this class was called RelayRequestError, and
    // `name` goes into logs and error reports. Changing it would only cause old and new records
    // to misalign, with no benefit.
    this.name = "RelayRequestError";
  }
}

/** The host explicitly rejected the request — as opposed to "the reply never arrived".
 *  Callers with fallback logic must only enable the fallback when this is false; otherwise
 *  they'll waste a whole grace window waiting for recovery from an error the host already
 *  responded to. */
export function isDesktopRejection(e: unknown): e is TransportError {
  return e instanceof TransportError && e.remote;
}

/** Timeout for resource requests (decision card preview images, wiki attachments).
 *
 *  Control requests are small, and the default of several seconds is more than enough; resource
 *  requests must move megabytes over potentially slow mobile links. Using the default value
 *  causes spurious timeouts on weak networks — pending entries are discarded, late replies are
 *  dropped, and `<img>` tags on the card hang forever with no error signal (see
 *  decisionAsset.test.ts and the corresponding e2e repro). */
export const ASSET_REQUEST_TIMEOUT_MS = 60_000;
/** Timeout budget for uploads. Same reasoning as above, just in the opposite direction and
 *  typically larger. */
export const UPLOAD_REQUEST_TIMEOUT_MS = 120_000;
/** Number of times `answerViaReq` will resend the reply before giving up. The host deduplicates
 *  by decision ID, so resends after a dropped reply are idempotent; this value caps how long
 *  to retry on weak networks before letting the card fall back to a retryable state. */
export const ANSWER_MAX_ATTEMPTS = 3;

/** One round trip, divided into segments with different root causes.
 *
 *  Looking only at `totalMs` doesn't tell whether the stall is the phone's network, the host's
 *  network, or the host's own handler — the three fixes are unrelated, so this breakdown is
 *  the entire point of the measurement.
 *
 *  Both segments are optional because either source can be absent: on a fast link, relay's
 *  `msg_ack` may arrive after the reply itself, and old hosts without `handle_ms` report nothing
 *  at all. Missing one segment just degrades the UI to a coarser answer; it doesn't invent one. */
export interface RttSample {
  /** Request to reply, measured by this phone's own clock for the entire round trip. */
  totalMs: number;
  /** Phone ↔ relay round trip. Same-origin HTTP has no relay leg, so always null. */
  phoneRelayMs: number | null;
  /** Time the host reports it spent in the handler (`handle_ms`), measured on the host's own
   *  clock, so no clock synchronization is involved. */
  desktopHandleMs: number | null;
}

/** Events pushed from the transport layer to the UI.
 *
 *  Each implementation must translate its own set of low-level signals into this callback set:
 *  relay translates WebSocket frames, HTTP implementations translate SSE events. Some signals
 *  have no counterpart in a given transport layer (in same-origin deployment, "is the host
 *  online" and "did the page load" are the same event), so let it remain constant or never
 *  fire — **don't fake a change**; the UI treats it as real. */
export interface TransportHandlers {
  /** Connectivity from this device to the data source. */
  onStatus?: (connected: boolean) => void;
  /** Connectivity on the host side. Under relay, this is "did the desktop connect to the
   *  relay"; under same-origin, the host is the process that sent this page, so it's online
   *  as long as the page is alive. */
  onAgentOnline?: (online: boolean) => void;
  onDecisionCreated?: (kind: DecisionKind, request: unknown) => void;
  onDecisionResolved?: (kind: DecisionKind, id: string) => void;
  onSessions?: (sessions: SessionInfo[]) => void;
  /** What kind of session frame just landed — `full` (entire snapshot) or `delta`
   *  (incremental additions/deletions). Lets the UI show whether the host's delta channel is
   *  really enabled. Fires on every session update. */
  onSessionsKind?: (kind: "full" | "delta") => void;
  /** A round-trip sample from request to reply. One of two weak-link congestion signals, and
   *  the only way to distinguish "link slow" from "host slow". */
  onRttSample?: (sample: RttSample) => void;
  /** Fires each time the connection drops and a reconnect is scheduled — the second weak-link
   *  signal (frequent reconnects ⇒ congestion). */
  onReconnect?: () => void;
  onAuthError?: (message: string) => void;
}

/** The small surface the view layer actually depends on.
 *
 *  Deliberately kept small: each additional method adds an obligation for the second
 *  implementation to either replicate or lie. Every item here has real call sites in the UI
 *  backing it. */
export interface FleetTransport {
  /** Start connecting / start receiving pushes. Idempotent. */
  connect(): void;
  /** Disconnect and stop all background activity. */
  close(): void;
  /** Best-effort "I'm leaving": lets the host remove this device without waiting for timeout. */
  sayGoodbye(): void;
  /** Whether the data plane is available. The UI uses this as a gate for polling and
   *  "is the link alive". */
  readonly isAuthed: boolean;
  /** Human-readable "what I'm connected to", for the "More" page to display one line.
   *
   *  On the interface instead of having the UI ask relay directly: the answer varies by
   *  transport layer (relay answers with the relay hostname, same-origin answers with its own
   *  origin), and the "More" page shouldn't have to import a concrete implementation just to
   *  display one line — that's exactly the kind of dependency that would drag relay into the
   *  same-origin build. */
  readonly endpointLabel: string;
  /** Send a data request to the host (pending_snapshot / task_plans / …). */
  request<T>(
    method: string,
    params?: Record<string, unknown>,
    timeoutMs?: number,
    onAck?: () => void,
    ackIsDelivery?: boolean,
  ): Promise<T>;
  /** Fire-and-forget reply. The boolean only means "sent", not "received by host". */
  answer(kind: DecisionKind, id: string, fields: Record<string, unknown>): boolean;
  /** Reliable reply path: get an actual delivery decision, resend if a frame is dropped (host
   *  deduplicates by decision ID, so resends are idempotent). Decision UI always uses this
   *  path to avoid having the card hang forever if a frame is lost. */
  answerViaReq(
    kind: DecisionKind,
    id: string,
    fields: Record<string, unknown>,
    opts?: { attempts?: number; timeoutMs?: number },
  ): Promise<void>;
  /** Register a push subscription. Returns false if the transport layer has no push channel —
   *  the caller uses this to decide whether to show the push toggle, so it must be honest
   *  "no", not pretend success. */
  pushSubscribe(subscription: unknown): boolean;
  /** Unregister a previously registered subscription. */
  pushUnsubscribe(subscription: unknown): boolean;
}
