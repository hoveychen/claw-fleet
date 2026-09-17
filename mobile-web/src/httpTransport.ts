// Same-origin HTTP transport: `fleet webui` serves this page and its data routes
// from the same port, so the "backend" lives at `window.location.origin` and requires
// no relay, paired keys, or WebSocket.
//
// **This file must never import relay.ts; this is a hard constraint, not a style
// preference.** When `relay.ts` loads, it executes `resolveRelayBase()` to resolve a
// relay address. Any import chain that touches it causes the browser build to carry a
// relay client it will never use, which breaks the premise that "webui doesn't depend
// on relay". Shared code lives in transport.ts (error tiers, timeout budgets, interfaces
// themselves), and that file is clean. `httpTransport.relay-free.test.ts` locks this
// constraint in via testing.
//
// Two channels correspond to two existing exits on the server side:
//
//   - **Requests** → `POST /mobile_rpc` (claw-fleet-core/src/routes.rs), bridged to
//     `mobile_relay::serve_request`. The method table is identical between relay frames
//     and requests here, so both transport layers receive byte-identical responses.
//   - **Pushes** → SSE on `GET /events`. The 2-second poll loop in hooks_server already
//     feeds both relay frames and SSE events simultaneously, sending the same `req`
//     serialized as JSON.
//
// One inconspicuous but critical fact about SSE: it is simultaneously a signal of
// **consumer presence**. hooks_server only writes `~/.fleet/consumer.heartbeat` when
// there is an SSE client (or a phone on relay). `fleet guard`, `fleet elicitation`,
// and `fleet mcp` check this heartbeat before blocking; if it's absent, they fall
// through directly to Claude Code's own terminal prompt. So an implementation that
// only polls without opening a stream doesn't just miss cards—it silently sends every
// prompt that should pop up on the phone back to the terminal instead. It must be a
// real EventSource.

import type { DecisionKind, SessionInfo } from "./types";
import { TransportError, type FleetTransport, type TransportHandlers } from "./transport";

/** SSE event name prefix → decision card kind.
 *
 *  The server sends two lines back-to-back: `sse.broadcast("guard-request", &json)`
 *  and `publish_decision_created("guard", v, ...)` (hooks_server/mod.rs). This table
 *  is not a guessed mapping but a transcription of that twin call's naming convention. */
const DECISION_KINDS: DecisionKind[] = [
  "guard",
  "elicitation",
  "fleet-ask",
  "plan-approval",
  "permission-prompt",
  "a2ui-render",
];

/** Default request timeout. Shared with relay implementation: timeout budget describes
 *  "how long someone is willing to wait", regardless of which path the bytes take. */
const REQUEST_TIMEOUT_MS = 15_000;

export interface HttpTransportOptions {
  /** Prefix for data routes. For same-origin deployment, leave it empty — `/mobile_rpc`
   *  is the root path, and the page parses correctly even when served under `/m/`. Tests
   *  use it to point to a stub.
   *
   *  "HTTP direct-to-host" entries in the device book give **absolute addresses**
   *  (cross-origin). The server must then return CORS headers, or the browser blocks it
   *  directly (see hooks_server in claw-fleet-core). */
  baseUrl?: string;
  /** Access token. Required when cross-origin direct-connecting to a host with a
   *  token gate.
   *
   *  The two channels carry tokens differently, and this is not a style issue:
   *  - Requests use the `Authorization: Bearer <t>` header.
   *  - SSE uses the `?token=<t>` query parameter — **EventSource cannot set request
   *    headers**; this is a hard browser API limit. The server recognizes both forms
   *    for this reason (the auth check in hooks_server/mod.rs explicitly states that
   *    "the latter is for SSE").
   *
   *  The cost is that the token appears in the SSE URL and may land in server access
   *  logs. This is a trade-off imposed by EventSource, not one we chose — we could
   *  avoid it by using fetch+ReadableStream to hand-write SSE, but that would require
   *  rewriting the entire streaming channel and reconnection semantics. */
  token?: string | null;
  fetchImpl?: typeof fetch;
  eventSourceImpl?: typeof EventSource;
}

/** Terminal state of `EventSource.readyState`. We use the literal value instead of
 *  `EventSource.CLOSED` because this class can be injected with `eventSourceImpl` (test
 *  stubs, non-browser hosts), and static constants may not exist in those contexts.
 *  The numeric value itself is fixed by the spec. */
const CLOSED = 2;

/** Backoff interval for reopening a stream. Shared with relay.ts values (start 1s,
 *  double, cap 15s): "how long before a link is worth retrying" is independent of
 *  whether it's WebSocket or SSE. */
const RECONNECT_BASE_MS = 1_000;
const RECONNECT_MAX_MS = 15_000;

export class HttpTransport implements FleetTransport {
  private stream: EventSource | null = null;
  private connected = false;
  private closed = false;
  private reconnectDelay = RECONNECT_BASE_MS;
  private reconnectTimer: number | undefined;

  constructor(
    private readonly handlers: TransportHandlers,
    private readonly opts: HttpTransportOptions = {},
  ) {}

  private get base(): string {
    return (this.opts.baseUrl ?? "").replace(/\/$/, "");
  }

  /** Token query string on the SSE URL. Empty string if no token is present. */
  private tokenQuery(): string {
    const token = this.opts.token;
    return token ? `?token=${encodeURIComponent(token)}` : "";
  }

  /** Request headers. When cross-origin direct-connecting with a token, add Bearer
   *  auth. For same-origin deployment without a token, only Content-Type is present,
   *  byte-for-byte identical to before. */
  private headers(): Record<string, string> {
    const h: Record<string, string> = { "Content-Type": "application/json" };
    if (this.opts.token) h.Authorization = `Bearer ${this.opts.token}`;
    return h;
  }

  connect(): void {
    if (this.stream) return;
    this.closed = false;
    const ES = this.opts.eventSourceImpl ?? globalThis.EventSource;
    const stream = new ES(`${this.base}/events${this.tokenQuery()}`);
    this.stream = stream;

    stream.onopen = () => {
      if (this.closed) return;
      this.connected = true;
      // Handshake success = this link is open now; the next disconnect should retry
      // from the shortest backoff.
      this.reconnectDelay = RECONNECT_BASE_MS;
      this.handlers.onStatus?.(true);
      // In same-origin deployment, "host is online" and "this page just loaded" are
      // the same thing: the process that served this page is the one answering
      // /mobile_rpc. There is no third-party relay to go down, so once this signal is
      // true it will not independently become false — it follows the connection itself.
      this.handlers.onAgentOnline?.(true);
    };

    // First-screen catch-up. **Can't rely on SSE alone**: the server's `sessions-updated`
    // only broadcasts when sessions change (`if sessions_changed` in hooks_server/mod.rs),
    // so a late-joining client that connects after nothing has changed will never receive
    // a frame and the task page will hang on "receiving first-screen snapshot". The relay
    // path has an extra condition for this: `|| mobile_clients > prev_mobile_clients`; SSE
    // has no equivalent.
    //
    // We fix this here rather than having the server re-push to new clients: SSE
    // broadcasts to all connections, and re-pushing the full snapshot for one new client
    // would disturb everyone else. Pulling once at mount is what HTTP clients should do
    // anyway — the desktop webui's liveProxy has always done it this way.
    void this.catchUpSessions();

    stream.onerror = () => {
      if (this.closed) return;
      if (this.connected) {
        this.connected = false;
        this.handlers.onStatus?.(false);
        this.handlers.onAgentOnline?.(false);
        this.handlers.onReconnect?.();
      }
      // EventSource's built-in retry **only covers network-layer disconnects**: when
      // readyState is stuck in CONNECTING, the browser will retry on its own, and if we
      // intervene we just open a second parallel stream (server counts an extra consumer,
      // events duplicate).
      //
      // But when the server returns non-200 (502/504 from a weak-network gateway) or
      // wrong Content-Type, the spec requires the UA to "fail the connection" —
      // readyState becomes CLOSED and **never retries**. Historical implementations
      // delegated all reconnection to that contract, so the stream dies there, and
      // `if (this.stream) return` at the start of connect() prevents anyone from
      // rebuilding it: only a page refresh works. There's a second entry point to the
      // same hole: the first handshake of never-before-connected (page opened while
      // offline) goes through here too, and old code's `!this.connected` early exit
      // suppressed it as well.
      if (stream.readyState === CLOSED) this.scheduleReopen();
    };

    for (const kind of DECISION_KINDS) {
      stream.addEventListener(`${kind}-request`, (e) => {
        const req = parseJson((e as MessageEvent).data);
        if (req !== undefined) this.handlers.onDecisionCreated?.(kind, req);
      });
      stream.addEventListener(`${kind}-dismissed`, (e) => {
        // The server sends `serde_json::to_string(id)` — a bare JSON string, not an
        // object. Parsing it as an object silently drops every "card dismissed" event,
        // and cards stay on the phone forever.
        const id = parseJson((e as MessageEvent).data);
        if (typeof id === "string") this.handlers.onDecisionResolved?.(kind, id);
      });
    }

    stream.addEventListener("sessions-updated", (e) => {
      const sessions = parseJson((e as MessageEvent).data);
      if (!Array.isArray(sessions)) return;
      this.handlers.onSessions?.(sessions as SessionInfo[]);
      // The server only sends full snapshots on this SSE stream. The relay path has
      // `sessions_delta`, but this one doesn't — reporting "delta" would make the UI
      // display a non-existent incremental link.
      this.handlers.onSessionsKind?.("full");
    });
  }

  /** Drop this dead stream and reopen one after backing off.
   *
   *  Add 0–30% jitter so N tabs don't all reconnect in the same millisecond when
   *  the network recovers. */
  private scheduleReopen(): void {
    if (this.closed || this.reconnectTimer !== undefined) return;
    const delay = this.reconnectDelay * (1 + Math.random() * 0.3);
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, RECONNECT_MAX_MS);
    this.reconnectTimer = globalThis.setTimeout(() => {
      this.reconnectTimer = undefined;
      if (this.closed) return;
      // connect() will early-exit if this.stream is non-null, so we must clear the
      // dead stream first.
      this.stream?.close();
      this.stream = null;
      this.connect();
    }, delay) as unknown as number;
  }

  /** Fetch a full sessions snapshot, compensating for the frame SSE won't replay for
   *  new clients.
   *
   *  Silently ignore failures: the SSE path may still deliver the data, and throwing
   *  here would turn a recoverable first-screen gap into a complete connection failure. */
  private async catchUpSessions(): Promise<void> {
    const fetchImpl = this.opts.fetchImpl ?? globalThis.fetch;
    try {
      const res = await fetchImpl(`${this.base}/sessions`);
      if (!res.ok) return;
      const sessions = await res.json();
      // It doesn't matter if we were close()'d or beaten by a real SSE frame — both
      // deliver full snapshots, the later one overwrites the earlier, and the result
      // is consistent.
      if (this.closed || !Array.isArray(sessions)) return;
      this.handlers.onSessions?.(sessions as SessionInfo[]);
      this.handlers.onSessionsKind?.("full");
    } catch {
      // See above: silent degradation.
    }
  }

  close(): void {
    this.closed = true;
    if (this.reconnectTimer !== undefined) {
      globalThis.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = undefined;
    }
    this.stream?.close();
    this.stream = null;
    if (this.connected) {
      this.connected = false;
      this.handlers.onStatus?.(false);
    }
  }

  /** Same-origin has no "device registry": the host doesn't maintain stale registrations
   *  for this page, so there's nothing to tear down proactively. An empty implementation
   *  is honest, not lazy. */
  sayGoodbye(): void {}

  get isAuthed(): boolean {
    return this.connected;
  }

  /** Display "where I connected to". For same-origin, it's the origin that served this
   *  page. For direct connection, it's that host's address (stripped of scheme and trailing
   *  slash, matching the relay convention). */
  get endpointLabel(): string {
    const base = this.base;
    if (!base) return globalThis.location?.host ?? "";
    try {
      const u = new URL(base);
      return u.protocol === "https:" ? u.host + u.pathname.replace(/\/$/, "") : base;
    } catch {
      return base;
    }
  }

  async request<T>(
    method: string,
    params?: Record<string, unknown>,
    timeoutMs?: number,
    onAck?: () => void,
    _ackIsDelivery?: boolean,
  ): Promise<T> {
    const fetchImpl = this.opts.fetchImpl ?? globalThis.fetch;
    // No relay in the middle, so "request received" and "request sent" are the same
    // moment. On the relay side, ack comes from the relay's managed receipt; here we
    // fire immediately to make behavior consistent for callers that depend on it for
    // UI feedback (immediate confirmation after submission).
    onAck?.();

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs ?? REQUEST_TIMEOUT_MS);
    let res: Response;
    try {
      res = await fetchImpl(`${this.base}/mobile_rpc`, {
        method: "POST",
        headers: this.headers(),
        body: JSON.stringify({ method, params: params ?? {} }),
        signal: controller.signal,
      });
    } catch (e) {
      // Network down, timeout abort, gateway rejection — we got no decision. The host
      // may have already done the work, so remote is false and the caller has the right
      // to double-check independently.
      throw new TransportError(errText(e), false);
    } finally {
      clearTimeout(timer);
    }

    if (!res.ok) {
      // HTTP-layer failures (502, 401, proxy timeout) also aren't the host's decision.
      throw new TransportError(`HTTP ${res.status}`, false);
    }

    let body: unknown;
    try {
      body = await res.json();
    } catch (e) {
      throw new TransportError(errText(e), false);
    }
    const reply = body as { ok?: boolean; data?: unknown; error?: string };
    if (reply?.ok !== true) {
      // The host received it, judged it, and said no. Retrying won't change the
      // outcome — remote is true, so the caller will skip its own grace fallback and
      // show the message directly to the user.
      throw new TransportError(reply?.error ?? "request failed", true);
    }
    return reply.data as T;
  }

  answer(kind: DecisionKind, id: string, fields: Record<string, unknown>): boolean {
    // Fire-and-forget old path. There's no "frame may be lost" problem here, so we take
    // the request path directly and discard the result — the boolean contract is just
    // "it was sent".
    void this.request("decision_answer", { kind, id, ...fields }).catch(() => {});
    return true;
  }

  async answerViaReq(
    kind: DecisionKind,
    id: string,
    fields: Record<string, unknown>,
    opts?: { attempts?: number; timeoutMs?: number },
  ): Promise<void> {
    // No retransmit. The relay-side retry handles "relay did best-effort delivery but
    // no one knows if frames were dropped". HTTP response itself is a delivery decision:
    // 2xx means the host confirmed, no response means truly failed — sending again just
    // replays the same failure.
    await this.request("decision_answer", { kind, id, ...fields }, opts?.timeoutMs);
  }

  /** This transport layer has no push channel: Web Push VAPID subscriptions live on
   *  the relay, and by design we don't touch relay here. Return false so the "More"
   *  page can hide the push toggle — pretending success would let users turn on the
   *  switch but never receive notifications, which is worse than having no switch. */
  pushSubscribe(_subscription: unknown): boolean {
    return false;
  }

  pushUnsubscribe(_subscription: unknown): boolean {
    return false;
  }
}

function parseJson(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
