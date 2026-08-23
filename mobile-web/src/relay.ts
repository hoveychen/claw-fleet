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
  /** Short git commit this bundle was built from (`__APP_COMMIT__`, see
   *  vite.config.ts). The desktop compares it against its own build commit and
   *  flags a device whose bundle is stale — the common "I redeployed the desktop
   *  but forgot to redeploy the relay" drift. `"unknown"` when the build had no
   *  commit source; the desktop then shows no version and raises no stale flag. */
  appCommit: string;
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
  /** Which kind of sessions frame just landed — `full` (whole snapshot) or
   *  `delta` (incremental upsert/remove). Lets the UI surface whether the
   *  desktop's delta path is actually engaged. Fired on every sessions update. */
  onSessionsKind?: (kind: "full" | "delta") => void;
  /** One request→reply round-trip time sample (ms), reported when a reply lands
   *  for a still-pending request. A weak-link congestion signal. */
  onRttSample?: (rttMs: number) => void;
  /** Fired each time the socket drops and a reconnect is scheduled — a second
   *  weak-link signal (frequent reconnects ⇒ congested). */
  onReconnect?: () => void;
  onAuthError?: (message: string) => void;
}

/** Base URL for HTTP endpoints (/vapid): dev override or same-origin. */
export function relayHttpBase(): string {
  return import.meta.env.VITE_RELAY_URL || window.location.origin;
}

/** Short, human-readable form of the relay this device talks to, for the More
 *  tab. `https` is the norm so its scheme is dropped as noise; anything else
 *  (a `http://127.0.0.1:…` dev relay) keeps the scheme, because that difference
 *  is exactly what you're looking at the row to find out.
 *
 *  Takes the base as an argument so it stays pure/testable; callers in the UI
 *  pass nothing and get the live one. */
export function relayDisplayHost(base: string = relayHttpBase()): string {
  try {
    const u = new URL(base);
    return u.protocol === "https:" ? u.host : `${u.protocol}//${u.host}`;
  } catch {
    return base;
  }
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
/** How many times `answerViaReq` sends the answer before giving up. The desktop
 *  dedups by decision id, so a resend after a lost reply is idempotent; this
 *  bounds how long a weak link is retried before the card reverts to retry. */
export const ANSWER_MAX_ATTEMPTS = 3;
/** How often the phone re-announces itself. The desktop drops a device ~40s
 *  after its last hello, so this must stay comfortably under that. */
const HELLO_INTERVAL_MS = 15_000;
/** A connection that stayed authed at least this long before dropping was
 *  genuinely healthy, so we reconnect fast (reset backoff to base). Shorter
 *  than this ⇒ the socket is flapping (auth → immediate drop), and resetting
 *  the backoff on every such auth would busy-loop reconnects once per second
 *  on a weak link. Keeping the backoff growing across flaps is the fix. */
const STABLE_CONNECTION_MS = 30_000;

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
      /** Epoch ms when request() sent this — used to compute RTT on reply. */
      sentAt: number;
      // 方案 A: fired once when the desktop's early `ack{req_id}` lands, before
      // the final reply — lets a caller confirm the submit reached the desktop
      // without waiting out the timeout. `acked` guards against a double fire.
      onAck?: () => void;
      acked?: boolean;
      /** Accept the relay's `msg_ack{queued}` as a successful outcome (decision
       *  answers only — see `request`'s doc comment). */
      ackIsDelivery?: boolean;
    }
  >();
  private reconnectDelay = 1000;
  /** Epoch ms of the last successful auth, or 0 if never/again unauthed. Used
   *  to tell a healthy long-lived connection from a flapping one on close. */
  private authedAt = 0;
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
      const stableMs = this.authed ? Date.now() - this.authedAt : 0;
      this.authed = false;
      this.authedAt = 0;
      this.stopHello();
      this.handlers.onStatus?.(false);
      this.failPending(t("连接已断开"));
      if (this.closed) return;
      // A drop that triggers a reconnect (not an intentional close) — surface it
      // as a congestion signal. Frequent reconnects ⇒ a flaky/weak link.
      this.handlers.onReconnect?.();
      // Only a connection that held for a while was genuinely healthy → reconnect
      // fast. A short-lived one is flapping; let the backoff keep growing so we
      // don't hammer a weak link once per second (the old `wasAuthed ? 1000`
      // reset did exactly that on every auth→drop cycle).
      if (stableMs >= STABLE_CONNECTION_MS) this.reconnectDelay = 1000;
      const delay = this.reconnectDelay;
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
        this.authedAt = Date.now();
        // NB: don't reset reconnectDelay here — a flapping socket auths every
        // cycle, so resetting on auth would defeat the backoff. The reset now
        // lives in onclose, gated on how long the connection actually held.
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
      case "msg_ack":
        this.handleMsgAck(String(frame.ack_id ?? ""), String(frame.status ?? ""));
        break;
      default:
        break; // notify frames are handled by the service worker via Web Push
    }
  }

  /** The relay's custody report for one of our outbound `msg` frames.
   *
   *  `queued` means no agent was online and the relay is holding the frame for
   *  the next one. For a decision answer that is as good as delivered: this
   *  phone's socket typically lives ~13s, far less than a desktop reconnect, so
   *  waiting for the desktop's own reply would fail an answer that is in fact
   *  safely in flight. It is NOT "the desktop acted on it" — the card is removed
   *  because the answer can no longer be lost, not because it was consumed.
   *
   *  `dropped` means nobody took it, so the caller should retry.
   *  `delivered` deliberately does nothing: the desktop is online and will send
   *  its own reply, which is the only thing that proves consumption. Treating it
   *  as done here would resurrect the optimistic-removal bug this path fixed. */
  private handleMsgAck(ackId: string, status: string) {
    const entry = this.pending.get(ackId);
    if (!entry) return;
    if (status === "queued" && entry.ackIsDelivery) {
      this.pending.delete(ackId);
      window.clearTimeout(entry.timer);
      entry.resolve(undefined);
      return;
    }
    if (status === "dropped") {
      this.pending.delete(ackId);
      window.clearTimeout(entry.timer);
      // remote=false → the caller may resend (the desktop dedups by decision id).
      entry.reject(new RelayRequestError(t("relay 未能转交（桌面离线）"), false));
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
        this.handlers.onSessionsKind?.("full");
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
        this.handlers.onSessionsKind?.("delta");
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
        this.handlers.onRttSample?.(Date.now() - entry.sentAt);
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
  private async sealAndSend(payload: unknown, ackId?: string): Promise<void> {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    await this.ensureKeys();
    const sealed = await seal(this.encKey!, JSON.stringify(payload));
    // `ack_id` rides *outside* the envelope on purpose: the relay can't open the
    // sealed body, so the `req_id` the endpoints correlate on is ciphertext to
    // it. This id is a meaningless random label whose only job is to let the
    // relay report back what it did with the frame (`msg_ack`).
    this.sendRaw(ackId ? { type: "msg", payload: sealed, ack_id: ackId } : { type: "msg", payload: sealed });
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

  /** Answer a decision — mirrors mobile_relay::handle_answer's shapes.
   *  Fire-and-forget: the boolean only reports the socket looked OPEN, NOT that
   *  the desktop received the answer. Kept for callers that don't need a verdict;
   *  the decision UI uses `answerViaReq` so a lost frame can't strand the card. */
  answer(kind: DecisionKind, id: string, fields: Record<string, unknown>): boolean {
    return this.sendPayload({ event: "answer", kind, id, ...fields });
  }

  /** Robust answer path: send the decision answer over the req/reply channel so
   *  the phone gets a real delivery verdict, and resend on a lost frame (the
   *  desktop dedups by decision id, so a resend is idempotent). Resolves once the
   *  desktop confirms delivery; rejects only after the resend budget is spent, or
   *  immediately on a desktop verdict (`ok:false`, e.g. the card already expired)
   *  — either way the caller keeps the card and lets the user retry rather than
   *  removing it optimistically the way `answer` used to force. */
  async answerViaReq(
    kind: DecisionKind,
    id: string,
    fields: Record<string, unknown>,
    opts?: { attempts?: number; timeoutMs?: number },
  ): Promise<void> {
    const attempts = Math.max(1, opts?.attempts ?? ANSWER_MAX_ATTEMPTS);
    let lastErr: unknown;
    for (let i = 0; i < attempts; i++) {
      try {
        // `ackIsDelivery`: a relay that took custody of the frame ends this
        // answer successfully even if the desktop is still offline.
        await this.request(
          "decision_answer",
          { kind, id, ...fields },
          opts?.timeoutMs,
          undefined,
          true,
        );
        return; // desktop confirmed delivery
      } catch (e) {
        lastErr = e;
        // A desktop verdict (ok:false) can't be changed by resending, so don't
        // resend. Only a lost frame / timeout / disconnect (remote === false) is
        // worth resending; the desktop dedups by decision id, so a resend that
        // races a delivered first attempt is an idempotent no-op.
        if (isDesktopRejection(e)) {
          // Forward-compat: a desktop that predates `decision_answer` rejects it
          // as an unknown method. Fall back to the legacy fire-and-forget `answer`
          // frame so a new phone can still answer an un-updated desktop — that
          // desktop only understands the old path anyway, so this is no worse
          // than its own behaviour, not a regression. Any other verdict (e.g. the
          // card already expired) is real: surface it. If even the fallback can't
          // send (socket down), keep the original error so the card is retained.
          if (/unknown method/i.test(e instanceof Error ? e.message : String(e))) {
            if (this.answer(kind, id, fields)) return;
          }
          throw e;
        }
      }
    }
    throw lastErr;
  }

  /** Data request to the agent (pending_snapshot / task_plans / …).
   *  `timeoutMs` overrides the default for slow methods (LLM analysis).
   *  `ackIsDelivery` opts this request into treating the relay's
   *  `msg_ack{status:"queued"}` as a successful outcome — only a decision answer
   *  wants that (see `answerViaReq`); a data request needs the desktop's actual
   *  payload, for which the relay's custody report is worthless. */
  request<T>(
    method: string,
    params?: Record<string, unknown>,
    timeoutMs?: number,
    onAck?: () => void,
    ackIsDelivery?: boolean,
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
        sentAt: Date.now(),
        onAck,
        ackIsDelivery,
      });
      // Only the body is encrypted; reqId/pending bookkeeping is unchanged. If
      // the seal/send fails (socket dropped mid-flight, crypto error), clear the
      // pending entry and reject now rather than sitting out the full timeout.
      void this.sealAndSend(
        { event: "req", req_id: reqId, method, params: params ?? {} },
        reqId,
      ).catch(
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
