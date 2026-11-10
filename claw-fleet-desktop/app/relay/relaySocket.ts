/**
 * Minimal relay client for the desktop web bundle served from a relay origin.
 *
 * Speaks the same envelope as `mobile-web/src/relay.ts` (auth → `msg` frames
 * carrying `{enc:"box"}` sealed payloads, `req`/`reply` correlation by
 * `req_id`) and seals with the very same code (`shared-ts/relayCrypto.ts`).
 * It is deliberately a *subset*: this page asks the Mac for `http` (the
 * tunnel) and `desktop_events` (the live-event lease, see `relayEvents.ts`),
 * and hands unsolicited payloads to `onPush` listeners. The phone's session
 * snapshots, decision pushes, presence hellos and push subscriptions are not
 * handled here; listeners filter for what they want.
 *
 * It does not announce itself with `client_hello`: the Mac's device list and
 * its gzip/delta capability gates are built from hellos, and a desktop tab
 * that never consumes snapshots should not take part in either.
 */

import { deriveKeys, isSealed, open, openBytes, seal, type SealedBox } from "../../../shared-ts/relayCrypto";

/** Default budget for one request. Tunnelled HTTP includes asset bytes. */
export const RELAY_REQUEST_TIMEOUT_MS = 30_000;
/** How long a request waits for a not-yet-authed socket before failing. */
const AUTH_GRACE_MS = 5_000;

/** A request the relay or the Mac could not complete. */
export class RelayError extends Error {
  /** `true` when the Mac answered `ok:false` (a verdict), `false` for a lost
   *  frame, timeout or a socket that is not connected. */
  readonly remote: boolean;
  constructor(message: string, remote: boolean) {
    super(message);
    this.remote = remote;
  }
}

export interface RelaySocketStatus {
  connected: boolean;
  /** Whether the relay reports the Mac (agent) as online in this channel. */
  agentOnline: boolean;
  /** Last auth error from the relay, if any. */
  error?: string;
}

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/** Minimal surface the tunnel needs; lets tests swap in a fake. */
export interface RelayRequester {
  request<T>(method: string, params: Record<string, unknown>, timeoutMs?: number): Promise<T>;
}

/** `https://relay.example` → `wss://relay.example/ws`. Same rule as mobile-web's `relayWsUrl`. */
export function relayWsUrl(base: string): string {
  return base.replace(/\/$/, "").replace(/^http/, "ws") + "/ws";
}

async function inflateGzip(buf: ArrayBuffer): Promise<string> {
  const stream = new Blob([buf]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Response(stream).text();
}

export class RelaySocket implements RelayRequester {
  private ws: WebSocket | null = null;
  private closed = false;
  private authed = false;
  private agentOnline = false;
  private reconnectDelay = 1000;
  private reqSeq = 0;
  // Replies are broadcast to every client in the channel (phones included), so
  // the id must be unique across devices, not just within this page.
  private readonly reqPrefix = crypto.randomUUID();
  private readonly pending = new Map<string, Pending>();
  private channelToken?: string;
  private encKey?: CryptoKey;
  private keysReady?: Promise<void>;
  /** Resolvers waiting for the first successful auth. */
  private authWaiters: Array<() => void> = [];
  private readonly pushListeners = new Set<(payload: Record<string, unknown>) => void>();
  private readonly statusListeners = new Set<(s: RelaySocketStatus) => void>();

  constructor(
    private readonly secret: string,
    private readonly base: string,
    private readonly onStatus?: (s: RelaySocketStatus) => void,
  ) {}

  private ensureKeys(): Promise<void> {
    if (!this.keysReady) {
      this.keysReady = deriveKeys(this.secret).then((k) => {
        this.channelToken = k.channelToken;
        this.encKey = k.encKey;
      });
    }
    return this.keysReady;
  }

  connect(): void {
    this.closed = false;
    void this.open();
  }

  close(): void {
    this.closed = true;
    this.ws?.close();
    this.ws = null;
  }

  get isAuthed(): boolean {
    return this.authed;
  }

  /** Resolves once the socket is authed, or rejects after `timeoutMs`. */
  waitAuthed(timeoutMs: number): Promise<void> {
    if (this.authed) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.authWaiters = this.authWaiters.filter((w) => w !== done);
        reject(new RelayError("relay not connected", false));
      }, timeoutMs);
      const done = () => {
        clearTimeout(timer);
        resolve();
      };
      this.authWaiters.push(done);
    });
  }

  get status(): RelaySocketStatus {
    return { connected: this.authed, agentOnline: this.agentOnline };
  }

  /** Every payload that is not a reply to one of this page's requests. */
  onPush(fn: (payload: Record<string, unknown>) => void): () => void {
    this.pushListeners.add(fn);
    return () => this.pushListeners.delete(fn);
  }

  /** Connection / agent presence changes, in addition to the constructor's. */
  onStatusChange(fn: (s: RelaySocketStatus) => void): () => void {
    this.statusListeners.add(fn);
    return () => this.statusListeners.delete(fn);
  }

  private emitStatus(error?: string) {
    const s = { connected: this.authed, agentOnline: this.agentOnline, error };
    this.onStatus?.(s);
    for (const fn of this.statusListeners) fn(s);
  }

  private async open() {
    if (this.closed) return;
    await this.ensureKeys();
    if (this.closed) return;
    const ws = new WebSocket(relayWsUrl(this.base));
    this.ws = ws;
    this.authed = false;
    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "auth", role: "client", secret: this.channelToken }));
    };
    ws.onmessage = (ev) => {
      let frame: Record<string, unknown>;
      try {
        frame = JSON.parse(String(ev.data));
      } catch {
        return;
      }
      this.handleFrame(frame);
    };
    ws.onclose = () => {
      const wasAuthed = this.authed;
      this.authed = false;
      this.failPending("relay connection closed");
      this.emitStatus();
      if (this.closed) return;
      if (wasAuthed) this.reconnectDelay = 1000;
      const delay = this.reconnectDelay * (1 + Math.random() * 0.3);
      this.reconnectDelay = Math.min(this.reconnectDelay * 2, 15_000);
      setTimeout(() => void this.open(), delay);
    };
    ws.onerror = () => ws.close();
  }

  private handleFrame(frame: Record<string, unknown>) {
    switch (frame.type) {
      case "authed": {
        this.authed = true;
        this.agentOnline = Boolean(frame.agent_online);
        this.emitStatus();
        const waiters = this.authWaiters;
        this.authWaiters = [];
        for (const w of waiters) w();
        break;
      }
      case "agent_status":
        this.agentOnline = Boolean(frame.online);
        this.emitStatus();
        break;
      case "error":
        this.emitStatus(String(frame.message ?? "relay auth failed"));
        break;
      case "msg_ack": {
        // `dropped`: no agent took the frame. A data request cannot be served
        // by custody alone, so fail it now rather than waiting out the timeout.
        if (frame.status !== "dropped") break;
        const id = String(frame.ack_id ?? "");
        const entry = this.pending.get(id);
        if (!entry) break;
        this.pending.delete(id);
        clearTimeout(entry.timer);
        entry.reject(new RelayError("relay could not deliver (Mac offline)", false));
        break;
      }
      case "msg": {
        const payload = (frame.payload ?? {}) as Record<string, unknown>;
        if (!isSealed(payload)) return;
        void this.openInbound(payload, payload.z === true)
          .then((json) => this.handlePayload(JSON.parse(json) as Record<string, unknown>))
          .catch(() => {});
        break;
      }
      default:
        break;
    }
  }

  private async openInbound(sealed: SealedBox, gzipped: boolean): Promise<string> {
    await this.ensureKeys();
    if (gzipped) return inflateGzip(await openBytes(this.encKey!, sealed));
    return open(this.encKey!, sealed);
  }

  private handlePayload(payload: Record<string, unknown>) {
    if (payload.event !== "reply") {
      for (const fn of this.pushListeners) fn(payload);
      return;
    }
    const reqId = String(payload.req_id ?? "");
    const entry = this.pending.get(reqId);
    if (!entry) return;
    this.pending.delete(reqId);
    clearTimeout(entry.timer);
    if (payload.ok) entry.resolve(payload.data);
    else entry.reject(new RelayError(String(payload.error ?? "request failed"), true));
  }

  async request<T>(method: string, params: Record<string, unknown>, timeoutMs?: number): Promise<T> {
    // Ride out a reconnect (or the very first auth) briefly rather than failing
    // every call made during it; a socket that stays down still fails fast.
    if (!this.authed) await this.waitAuthed(AUTH_GRACE_MS);
    const reqId = `${this.reqPrefix}-${++this.reqSeq}`;
    return new Promise<T>((resolve, reject) => {
      const ws = this.ws;
      if (!ws || ws.readyState !== WebSocket.OPEN || !this.authed) {
        reject(new RelayError("relay not connected", false));
        return;
      }
      const timer = setTimeout(() => {
        this.pending.delete(reqId);
        reject(new RelayError("relay request timed out (Mac may be offline)", false));
      }, timeoutMs ?? RELAY_REQUEST_TIMEOUT_MS);
      this.pending.set(reqId, { resolve: resolve as (v: unknown) => void, reject, timer });
      void (async () => {
        await this.ensureKeys();
        const sealed = await seal(
          this.encKey!,
          JSON.stringify({ event: "req", req_id: reqId, method, params }),
        );
        // `ack_id` outside the envelope so the relay can report custody.
        ws.send(JSON.stringify({ type: "msg", payload: sealed, ack_id: reqId }));
      })().catch(() => {
        const entry = this.pending.get(reqId);
        if (!entry) return;
        this.pending.delete(reqId);
        clearTimeout(entry.timer);
        entry.reject(new RelayError("relay not connected", false));
      });
    });
  }

  private failPending(message: string) {
    for (const [, entry] of this.pending) {
      clearTimeout(entry.timer);
      entry.reject(new RelayError(message, false));
    }
    this.pending.clear();
  }
}
