// WebSocket client for fleet-relay. Speaks the relay envelope
// (auth/msg/notify/presence/agent_status) and the Fleet business frames
// inside msg.payload (decision_created / decision_resolved / sessions /
// answer / req / ack / reply) — see claw-fleet-core/src/mobile_relay.rs.

import { t } from "./i18n";
import { deriveKeys, isSealed, open, openBytes, seal, type SealedBox } from "./relayCrypto";
import type { DecisionKind, SessionInfo } from "./types";

/** Self-description this phone announces to the desktop so it appears in the
 *  desktop 「移动端」 device list. Provided lazily so `pushSubscribed` reflects
 *  the state at each heartbeat, not just at construction. */
export interface DeviceInfo {
  clientId: string;
  label: string;
  platform: string;
  pushSubscribed: boolean;
  /** Whether this browser can inflate a gzipped payload via the native
   *  `DecompressionStream`. The desktop only compresses when every live client
   *  reports `true`, so a browser without the API keeps receiving plaintext. */
  supportsGzip: boolean;
  /** Vestigial. Under always-on end-to-end encryption every `msg` payload is a
   *  sealed `{enc:"box"}` text envelope, so the old raw-binary / `{enc:"gzip"}`
   *  transports are gone and nothing gates on this. Still reported so a desktop
   *  that reads the field stays wire-compatible. */
  supportsBinary: boolean;
  /** Whether this client applies `sessions_delta` frames (keyed upsert/remove)
   *  instead of needing a whole-list re-push on every change. Pure JS — no
   *  browser API required — so it's always `true` here; the desktop only emits
   *  deltas when *every* live client reports `true`, falling back to full
   *  snapshots otherwise. Absent → the desktop treats the client as legacy. */
  supportsDelta: boolean;
}

/** A failed `request()`, tagged with *where* the failure came from.
 *
 *  `remote: true` — the desktop received the request, judged it, and said no
 *  (an `ok:false` reply). The message is the desktop's own text. Retrying or
 *  waiting changes nothing; show it to the user now.
 *
 *  `remote: false` — the request never got a verdict: it timed out, the socket
 *  was down, or the reply frame was dropped (the relay forwards best-effort, no
 *  queue, no retry — a phone that switched networks mid-flight never sees it).
 *  The desktop may well have done the work, so the caller is entitled to
 *  confirm out-of-band before declaring failure (see `waitForSessionId`). */
export class RelayRequestError extends Error {
  constructor(
    message: string,
    readonly remote: boolean,
  ) {
    super(message);
    this.name = "RelayRequestError";
  }
}

/** True when the desktop explicitly rejected the request — as opposed to the
 *  reply never arriving. Callers that have a grace-period fallback must gate it
 *  on this being false, or they will sit out the whole window on an error the
 *  desktop already handed them. */
export function isDesktopRejection(e: unknown): e is RelayRequestError {
  return e instanceof RelayRequestError && e.remote;
}

/** Native gzip inflation support (Safari 16.4+, all evergreen Chrome/Firefox).
 *  Announced to the desktop in `client_hello` so it can gate compression. */
export function gzipSupported(): boolean {
  return typeof DecompressionStream !== "undefined";
}

/** Vestigial companion to `supportsBinary` (see `DeviceInfo`). Retained so the
 *  `client_hello` field keeps a truthful value; no transport depends on it. */
export function binarySupported(): boolean {
  return gzipSupported();
}

/** Inflate raw gzip bytes (the plaintext of a sealed `z:true` payload) into its
 *  JSON string, using the streaming DecompressionStream API. */
async function inflateGzipBytes(buf: ArrayBuffer): Promise<string> {
  const stream = new Blob([buf]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Response(stream).text();
}

export interface RelayHandlers {
  /** WS-level connectivity (this phone ↔ relay). */
  onStatus?: (connected: boolean) => void;
  /** Agent-level connectivity (desktop ↔ relay). */
  onAgentOnline?: (online: boolean) => void;
  onDecisionCreated?: (kind: DecisionKind, request: unknown) => void;
  onDecisionResolved?: (kind: DecisionKind, id: string) => void;
  onSessions?: (sessions: SessionInfo[]) => void;
  onAuthError?: (message: string) => void;
}

/** Base URL for HTTP endpoints (/vapid): dev override or same-origin. */
export function relayHttpBase(): string {
  return import.meta.env.VITE_RELAY_URL || window.location.origin;
}

function relayWsUrl(): string {
  const base = relayHttpBase().replace(/\/$/, "");
  return base.replace(/^http/, "ws") + "/ws";
}

const REQUEST_TIMEOUT_MS = 15_000;
/** Control messages (snapshots, tails, marks) are small and 15s is plenty.
 *  Asset/upload requests move MB-scale base64 across a possibly-slow mobile
 *  link, so the default would spuriously abort on weak connections — the
 *  pending entry is dropped, the late reply discarded, and the card's <img>
 *  strands forever with no error (see decisionAsset.test.ts / the e2e repro).
 *  These give the payload a realistic window instead. */
export const ASSET_REQUEST_TIMEOUT_MS = 60_000;
export const UPLOAD_REQUEST_TIMEOUT_MS = 120_000;
/** How often the phone re-announces itself. The desktop drops a device ~40s
 *  after its last hello, so this must stay comfortably under that. */
const HELLO_INTERVAL_MS = 15_000;

export class RelayClient {
  private ws: WebSocket | null = null;
  private secret: string;
  private handlers: RelayHandlers;
  private deviceInfo?: () => DeviceInfo;
  private helloTimer: number | null = null;
  // Last full sessions snapshot, kept so `sessions_delta` frames can be applied
  // on top of it (keyed upsert/remove). A full `sessions` frame replaces it.
  private sessionsSnapshot: SessionInfo[] = [];
  private reqSeq = 0;
  // Per-instance prefix so req_ids never collide across devices sharing a
  // channel: the relay broadcasts every reply to all clients (registry.rs
  // forward), so a bare counter (r1, r2…) would let one phone's reply match
  // another's identically-numbered pending. See relay.test.ts.
  private reqPrefix = crypto.randomUUID();
  private pending = new Map<
    string,
    {
      resolve: (v: unknown) => void;
      reject: (e: Error) => void;
      timer: number;
      // 方案 A: fired once when the desktop's early `ack{req_id}` lands, before
      // the final reply — lets a caller confirm the submit reached the desktop
      // without waiting out the timeout. `acked` guards against a double fire.
      onAck?: () => void;
      acked?: boolean;
    }
  >();
  private reconnectDelay = 1000;
  private closed = false;
  private authed = false;
  // End-to-end encryption keys derived from the pairing secret (方案A). The
  // relay only ever sees `channelToken` (what we auth with) and sealed
  // ciphertext; the raw secret and `encKey` never leave this device. Derivation
  // is async (WebCrypto), so it's memoized in `keysReady` and awaited before the
  // first socket opens and before every seal/open.
  private channelToken?: string;
  private encKey?: CryptoKey;
  private keysReady?: Promise<void>;

  constructor(secret: string, handlers: RelayHandlers, deviceInfo?: () => DeviceInfo) {
    this.secret = secret;
    this.handlers = handlers;
    this.deviceInfo = deviceInfo;
  }

  /** Derive (once) the channel token and AES key from the pairing secret. */
  private ensureKeys(): Promise<void> {
    if (!this.keysReady) {
      this.keysReady = deriveKeys(this.secret).then((k) => {
        this.channelToken = k.channelToken;
        this.encKey = k.encKey;
      });
    }
    return this.keysReady;
  }

  connect() {
    this.closed = false;
    void this.open();
  }

  close() {
    this.closed = true;
    this.sayGoodbye();
    this.stopHello();
    this.ws?.close();
    this.ws = null;
  }

  /** Best-effort "I'm leaving" so the desktop drops this device without waiting
   *  out the stale timeout. Fire from `pagehide`/`beforeunload` too — mobile
   *  browsers may never run cleanup, hence the desktop-side timeout backstop. */
  sayGoodbye() {
    const info = this.deviceInfo?.();
    if (info) this.sendPayload({ event: "client_bye", clientId: info.clientId });
  }

  private startHello() {
    this.sendHello();
    this.stopHello();
    this.helloTimer = window.setInterval(() => this.sendHello(), HELLO_INTERVAL_MS);
  }

  private stopHello() {
    if (this.helloTimer !== null) {
      window.clearInterval(this.helloTimer);
      this.helloTimer = null;
    }
  }

  private sendHello() {
    const info = this.deviceInfo?.();
    if (info) this.sendPayload({ event: "client_hello", ...info });
  }

  get isAuthed(): boolean {
    return this.authed;
  }

  private async open() {
    if (this.closed) return;
    // Keys must exist before we auth: the `auth` frame carries `channelToken`,
    // not the raw secret. Memoized, so reconnects resolve instantly.
    await this.ensureKeys();
    if (this.closed) return; // close() may have fired while deriving
    const ws = new WebSocket(relayWsUrl());
    this.ws = ws;
    this.authed = false;
    ws.onopen = () => {
      // Auth with the HKDF-derived channel token, never the raw secret — the
      // relay routes by a one-way function of the secret and never learns it.
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
      this.stopHello();
      this.handlers.onStatus?.(false);
      this.failPending(t("连接已断开"));
      if (this.closed) return;
      const delay = wasAuthed ? 1000 : this.reconnectDelay;
      this.reconnectDelay = Math.min(this.reconnectDelay * 2, 15_000);
      window.setTimeout(() => void this.open(), delay);
    };
    ws.onerror = () => {
      ws.close();
    };
  }

  private handleFrame(frame: Record<string, unknown>) {
    switch (frame.type) {
      case "authed":
        this.authed = true;
        this.reconnectDelay = 1000;
        this.handlers.onStatus?.(true);
        this.handlers.onAgentOnline?.(Boolean(frame.agent_online));
        this.startHello();
        break;
      case "agent_status":
        this.handlers.onAgentOnline?.(Boolean(frame.online));
        break;
      case "error":
        this.handlers.onAuthError?.(String(frame.message ?? t("认证失败")));
        break;
      case "msg":
        this.dispatchMsgPayload((frame.payload ?? {}) as Record<string, unknown>);
        break;
      default:
        break; // notify frames are handled by the service worker via Web Push
    }
  }

  /** A `msg` frame's payload is a sealed `{enc:"box",iv,ct,z?}` envelope under
   *  always-on encryption: open it, inflate if it was gzipped before sealing
   *  (`z:true`), then dispatch the JSON business payload. Anything that isn't a
   *  well-formed sealed envelope can't have come from our paired desktop, so
   *  it's dropped (the next full snapshot self-heals, and a lost reply just
   *  times out like any dropped frame). */
  private dispatchMsgPayload(payload: Record<string, unknown>) {
    const gzipped = payload.z === true;
    if (!isSealed(payload)) return;
    void this.openInbound(payload, gzipped)
      .then((json) => this.handlePayload(JSON.parse(json) as Record<string, unknown>))
      .catch(() => {});
  }

  /** Decrypt a sealed inbound payload into its JSON string, inflating first when
   *  the desktop gzipped it before sealing (`z:true` — the plaintext is then raw
   *  gzip bytes, so it must be opened at the byte level, not through a decoder). */
  private async openInbound(sealed: SealedBox, gzipped: boolean): Promise<string> {
    await this.ensureKeys();
    if (gzipped) {
      const bytes = await openBytes(this.encKey!, sealed);
      return inflateGzipBytes(bytes);
    }
    return open(this.encKey!, sealed);
  }

  private handlePayload(payload: Record<string, unknown>) {
    switch (payload.event) {
      case "decision_created":
        this.handlers.onDecisionCreated?.(
          payload.kind as DecisionKind,
          payload.request,
        );
        break;
      case "decision_resolved":
        // Diagnostic: whether the live resolved broadcast actually lands here
        // is the open question behind the "desktop-answered card won't dismiss
        // on mobile" bug. If this logs but the card lingered, the fault is in
        // acting on it; if it never logs during a repro, the frame isn't
        // arriving. The periodic reconcile in App.tsx is the fallback either way.
        console.debug("[decision] resolved recv", payload.kind, payload.id);
        this.handlers.onDecisionResolved?.(
          payload.kind as DecisionKind,
          String(payload.id ?? ""),
        );
        break;
      case "sessions": {
        // Whole-payload compression is handled at the envelope (`z`) in
        // openInbound, so by here the snapshot is already plain JSON. A full
        // snapshot is the delta baseline: replace local state wholesale.
        const list = (payload.sessions ?? []) as SessionInfo[];
        this.sessionsSnapshot = list.slice();
        this.handlers.onSessions?.(list);
        break;
      }
      case "sessions_delta": {
        // Incremental update against the last full snapshot: apply removals then
        // upserts keyed by id, and re-sort by lastActivityMs desc so ordering
        // matches the desktop's full-snapshot order. onSessions still receives
        // the whole merged list, so the UI is unaware deltas are in play.
        const upsert = (payload.upsert ?? []) as SessionInfo[];
        const remove = new Set((payload.remove ?? []) as string[]);
        const byId = new Map(this.sessionsSnapshot.map((s) => [s.id, s]));
        for (const id of remove) byId.delete(id);
        for (const s of upsert) byId.set(s.id, s);
        const merged = [...byId.values()].sort(
          (a, b) => (b.lastActivityMs ?? 0) - (a.lastActivityMs ?? 0),
        );
        this.sessionsSnapshot = merged;
        this.handlers.onSessions?.(merged);
        break;
      }
      case "ack": {
        // Early submit-ack (方案 A): the desktop received a write req and is now
        // processing it. Fire the caller's onAck once; the promise still settles
        // on the eventual reply / timeout.
        const reqId = String(payload.req_id ?? "");
        const entry = this.pending.get(reqId);
        if (entry?.onAck && !entry.acked) {
          entry.acked = true;
          entry.onAck();
        }
        break;
      }
      case "reply": {
        const reqId = String(payload.req_id ?? "");
        const entry = this.pending.get(reqId);
        if (!entry) return;
        this.pending.delete(reqId);
        window.clearTimeout(entry.timer);
        if (payload.ok) {
          entry.resolve(payload.data);
        } else {
          // A verdict from the desktop — not a lost frame. See RelayRequestError.
          entry.reject(new RelayRequestError(String(payload.error ?? t("请求失败")), true));
        }
        break;
      }
      default:
        break;
    }
  }

  private sendRaw(frame: unknown): boolean {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return false;
    this.ws.send(JSON.stringify(frame));
    return true;
  }

  /** Seal a business payload and send it as a `msg` frame. The phone's outbound
   *  payloads (answer/req/hello/bye) are small, so it never gzips before sealing
   *  — `z` is only ever set by the desktop. Best-effort: a closed socket or a
   *  seal failure drops the frame (the desktop's next snapshot reconciles). */
  private async sealAndSend(payload: unknown): Promise<void> {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    await this.ensureKeys();
    const sealed = await seal(this.encKey!, JSON.stringify(payload));
    this.sendRaw({ type: "msg", payload: sealed });
  }

  /** Fire-and-forget seal+send shell so the synchronous callers (answer / hello
   *  / bye) keep their boolean signature. The boolean reports whether the socket
   *  was open at call time; the encrypted send itself completes on a later
   *  microtask (sealing is async). */
  private sendPayload(payload: unknown): boolean {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return false;
    void this.sealAndSend(payload);
    return true;
  }

  /** Answer a decision — mirrors mobile_relay::handle_answer's shapes. */
  answer(kind: DecisionKind, id: string, fields: Record<string, unknown>): boolean {
    return this.sendPayload({ event: "answer", kind, id, ...fields });
  }

  /** Data request to the agent (pending_snapshot / task_plans / …).
   *  `timeoutMs` overrides the default for slow methods (LLM analysis). */
  request<T>(
    method: string,
    params?: Record<string, unknown>,
    timeoutMs?: number,
    onAck?: () => void,
  ): Promise<T> {
    const reqId = `${this.reqPrefix}-${++this.reqSeq}`;
    return new Promise<T>((resolve, reject) => {
      // Sealing is async, so probe the socket up front (not via sendPayload's
      // return) to fail-fast the "not connected" case before registering pending.
      if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
        reject(new RelayRequestError(t("尚未连接 relay"), false));
        return;
      }
      const timer = window.setTimeout(() => {
        this.pending.delete(reqId);
        reject(new RelayRequestError(t("请求超时（桌面端可能离线）"), false));
      }, timeoutMs ?? REQUEST_TIMEOUT_MS);
      this.pending.set(reqId, {
        resolve: resolve as (v: unknown) => void,
        reject,
        timer,
        onAck,
      });
      // Only the body is encrypted; reqId/pending bookkeeping is unchanged. If
      // the seal/send fails (socket dropped mid-flight, crypto error), clear the
      // pending entry and reject now rather than sitting out the full timeout.
      void this.sealAndSend({ event: "req", req_id: reqId, method, params: params ?? {} }).catch(
        () => {
          const entry = this.pending.get(reqId);
          if (!entry) return;
          this.pending.delete(reqId);
          window.clearTimeout(entry.timer);
          entry.reject(new RelayRequestError(t("尚未连接 relay"), false));
        },
      );
    });
  }

  /** Register a Web Push subscription on this channel. */
  pushSubscribe(subscription: unknown): boolean {
    return this.sendRaw({ type: "push_subscribe", subscription });
  }

  /** Remove a previously registered subscription from this channel. The relay
   *  matches a web sub by `endpoint` (harmony by `openId`), so the same shape
   *  passed to `pushSubscribe` works here too. */
  pushUnsubscribe(subscription: unknown): boolean {
    return this.sendRaw({ type: "push_unsubscribe", subscription });
  }

  private failPending(message: string) {
    for (const [, entry] of this.pending) {
      window.clearTimeout(entry.timer);
      // The socket dropped while these were in flight: no verdict, so callers
      // with an out-of-band confirmation path may still use it.
      entry.reject(new RelayRequestError(message, false));
    }
    this.pending.clear();
  }
}
