import { connection, webSocket } from '@kit.NetworkKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { util } from '@kit.ArkTS';
import { deriveKeys, isSealed, openBytes, openString, sealString, SealedBox } from './RelayCrypto';
import { gunzip } from './Gzip';
import { SessionInfo } from '../models/Types';

/// Persistent WebSocket client for fleet-relay — the native ArkTS peer of
/// mobile-web/src/relay.ts. Speaks the relay envelope (auth / msg / agent_status)
/// and the Fleet business frames sealed inside `msg.payload`
/// (decision_created / decision_resolved / sessions / answer / req / reply); see
/// claw-fleet-core/src/mobile_relay.rs.
///
/// Encryption is opaque here: every `msg` payload is a sealed `{enc:"box"}`
/// envelope handled via RelayCrypto.sealString/openString. We advertise
/// `supportsGzip` (once Gzip.gzipSelfTest passes), and inflate `z:true`
/// payloads via Gzip.gunzip — the desktop compresses only when EVERY live
/// client opts in, so this capability is what keeps the channel compressed.

const DOMAIN = 0x0000;
const TAG = 'fleet';

const REQUEST_TIMEOUT_MS = 15000;
const HELLO_INTERVAL_MS = 15000;
const MAX_RECONNECT_MS = 15000;
// A connect attempt that neither auths nor errors within this window is
// declared wedged and torn down. Device-observed failure mode (2026-07-15,
// LMR-AL00): with the network gone, ws.connect() never invokes its callback
// and no 'close'/'error' event ever fires — without this watchdog the client
// sits in "连接中" forever even after the network returns.
const CONNECT_ATTEMPT_TIMEOUT_MS = 10000;

/// ArkTS bans untyped object literals (`{}`) and object literals for `Record`
/// types, so we use an empty class instance wherever a "no data" / "no params"
/// placeholder object is needed on the wire.
class EmptyObj {}

/// Self-description this device announces so it appears in the desktop's device
/// list. Mirrors mobile-web's DeviceInfo.
export interface DeviceInfo {
  clientId: string;
  label: string;
  platform: string;
  pushSubscribed: boolean;
  supportsGzip: boolean;
  supportsBinary: boolean;
}

export interface RelayHandlers {
  onStatus?: (connected: boolean) => void;
  onAgentOnline?: (online: boolean) => void;
  onDecisionCreated?: (kind: string, request: Object) => void;
  onDecisionResolved?: (kind: string, id: string) => void;
  onSessions?: (sessions: SessionInfo[]) => void;
  onAuthError?: (message: string) => void;
}

// ── Wire frames (declared as interfaces so JSON.parse casts are typed) ─────────

interface InboundFrame {
  type?: string;
  agent_online?: boolean;
  online?: boolean;
  message?: string;
  payload?: SealedPayload;
}

interface SealedPayload {
  enc?: string;
  iv?: string;
  ct?: string;
  z?: boolean;
}

interface BusinessPayload {
  event?: string;
  kind?: string;
  id?: string;
  request?: Object;
  sessions?: SessionInfo[];
  req_id?: string;
  ok?: boolean;
  data?: Object;
  error?: string;
}

interface AuthFrame {
  type: string;
  role: string;
  secret: string;
}

interface MsgFrame {
  type: string;
  payload: SealedBox;
}

interface PendingEntry {
  resolve: (v: Object) => void;
  // Typed as Object (not Error): ArkTS's arkts-no-structural-typing rejects
  // passing a RelayRequestError to an `(e: Error)` param, but every value IS-A
  // Object nominally, so this is always assignable. The rejected value is still
  // a real Error subclass at runtime (instanceof Error holds).
  reject: (e: Object) => void;
  timer: number;
  // 方案 A:桌面早 ack{req_id} 到达时触发一次(提交已抵达桌面),promise 仍待最终
  // reply 才 settle。acked 防重复触发。
  onAck?: () => void;
  acked?: boolean;
}

/// A failed `request`, tagged with whether the desktop actually rejected it
/// (`remote:true`) or the reply just never arrived (`remote:false`).
export class RelayRequestError extends Error {
  remote: boolean;

  constructor(message: string, remote: boolean) {
    super(message);
    this.name = 'RelayRequestError';
    this.remote = remote;
  }
}

/// True when the desktop explicitly rejected the request (an `ok:false` reply) —
/// as opposed to the reply never arriving. Callers with a grace-period / resend
/// fallback must gate it on this being false, or they sit out the whole window /
/// resend on an error the desktop already handed them. 对齐 mobile-web relay.ts。
export function isDesktopRejection(e: Object): boolean {
  return e instanceof RelayRequestError && (e as RelayRequestError).remote;
}

function toWsUrl(relayUrl: string): string {
  let base = relayUrl.replace(/\/+$/, '');
  if (base.startsWith('https://')) {
    base = 'wss://' + base.substring('https://'.length);
  } else if (base.startsWith('http://')) {
    base = 'ws://' + base.substring('http://'.length);
  }
  return base + '/ws';
}

export class RelayClient {
  private relayUrl: string;
  private secret: string;
  private handlers: RelayHandlers;
  private deviceInfoProvider?: () => DeviceInfo;
  private ws: webSocket.WebSocket | null = null;
  private helloTimer: number = -1;
  private reqSeq = 0;
  // Per-instance prefix so req_ids never collide across devices sharing a
  // channel (the relay broadcasts every reply to all clients).
  private reqPrefix: string = util.generateRandomUUID(true);
  private pending: Map<string, PendingEntry> = new Map<string, PendingEntry>();
  private reconnectDelay = 1000;
  private closed = false;
  // Idempotence guard: 'close' events, 'error' events, connect-callback errors
  // and the attempt watchdog can all fire for one dead socket; only the first
  // schedules the retry.
  private reconnectPending = false;
  private reconnectTimer = -1;
  // Monotonic connect-attempt id so a stale watchdog from attempt N can never
  // kill attempt N+1.
  private attemptSeq = 0;
  private netConn: connection.NetConnection | null = null;
  private authed = false;
  private channelToken?: string;
  private encKey?: Uint8Array;
  private keysReady?: Promise<void>;

  constructor(relayUrl: string, secret: string, handlers: RelayHandlers,
    deviceInfoProvider?: () => DeviceInfo) {
    this.relayUrl = relayUrl;
    this.secret = secret;
    this.handlers = handlers;
    this.deviceInfoProvider = deviceInfoProvider;
  }

  get isAuthed(): boolean {
    return this.authed;
  }

  /// Derive (once) the channel token and AES key from the pairing secret.
  private ensureKeys(): Promise<void> {
    let ready = this.keysReady;
    if (!ready) {
      ready = deriveKeys(this.secret).then((k) => {
        this.channelToken = k.channelToken;
        this.encKey = k.encKey;
      });
      this.keysReady = ready;
    }
    return ready;
  }

  connect(): void {
    this.closed = false;
    this.startNetWatch();
    this.open();
  }

  /// Mirror of the browser's platform behaviour the web client gets for free:
  /// the OS tells us the network came/went, we tear down and redial instead of
  /// trusting a zombie socket.
  private startNetWatch(): void {
    if (this.netConn) {
      return;
    }
    try {
      const conn = connection.createNetConnection();
      conn.register((err: BusinessError) => {
        if (err) {
          hilog.warn(DOMAIN, TAG, 'netwatch register failed: %{public}s', JSON.stringify(err));
        }
      });
      conn.on('netAvailable', () => {
        hilog.info(DOMAIN, TAG, 'netwatch: netAvailable (authed=%{public}d)', this.authed ? 1 : 0);
        if (!this.authed) {
          this.kickNow('network available');
        }
      });
      conn.on('netLost', () => {
        hilog.warn(DOMAIN, TAG, 'netwatch: netLost');
        this.forceReconnect('network lost');
      });
      conn.on('netUnavailable', () => {
        hilog.warn(DOMAIN, TAG, 'netwatch: netUnavailable');
        this.forceReconnect('network unavailable');
      });
      this.netConn = conn;
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'netwatch unavailable: %{public}s', JSON.stringify(e));
    }
  }

  private stopNetWatch(): void {
    const conn = this.netConn;
    this.netConn = null;
    if (conn) {
      try {
        conn.unregister((_e: BusinessError) => {});
      } catch (_e) {
        // best-effort
      }
    }
  }

  /// Drop whatever socket/backoff state exists and redial after ~1s. Public so
  /// the ability's onForeground hook can nudge a client whose timers were
  /// frozen while backgrounded.
  kickNow(reason: string): void {
    if (this.closed || this.authed) {
      return;
    }
    if (this.reconnectPending) {
      clearTimeout(this.reconnectTimer);
      this.reconnectPending = false;
    }
    this.reconnectDelay = 1000;
    this.forceReconnect(reason);
  }

  /// Tear down the current socket (if any) and schedule a reconnect. Safe to
  /// call from every failure path. Teardown runs UNCONDITIONALLY — the hello
  /// interval and `authed` must die with the socket even when a retry is
  /// already queued, or a stale hello fires on the next socket before its
  /// `auth` frame and the relay kills the connection with "expected auth
  /// frame" (which the app used to surface as 配对失败).
  private forceReconnect(reason: string): void {
    if (this.closed) {
      return;
    }
    hilog.warn(DOMAIN, TAG, 'relay reconnect (%{public}s)', reason);
    const wasAuthed = this.authed;
    this.teardownSocket();
    this.scheduleReconnect(wasAuthed);
  }

  /// Kill the current socket + all per-connection state (hello timer, authed
  /// flag, pending requests). Idempotent.
  private teardownSocket(): void {
    this.authed = false;
    this.stopHello();
    this.handlers.onStatus?.(false);
    this.failPending('连接已断开');
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      try {
        ws.close((_e: BusinessError, _ok: boolean) => {});
      } catch (_e) {
        // already dead
      }
    }
  }

  /// Queue a single retry (multiple failure paths may fire for one death).
  private scheduleReconnect(wasAuthed: boolean): void {
    if (this.closed || this.reconnectPending) {
      return;
    }
    const delay = wasAuthed ? 1000 : this.reconnectDelay;
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, MAX_RECONNECT_MS);
    this.reconnectPending = true;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectPending = false;
      this.open();
    }, delay);
  }

  close(): void {
    this.closed = true;
    this.stopNetWatch();
    if (this.reconnectPending) {
      clearTimeout(this.reconnectTimer);
      this.reconnectPending = false;
    }
    this.sayGoodbye();
    this.stopHello();
    if (this.ws) {
      this.ws.close((_e: BusinessError, _ok: boolean) => {});
      this.ws = null;
    }
  }

  /// Best-effort "I'm leaving" so the desktop drops this device promptly.
  sayGoodbye(): void {
    const info = this.deviceInfoProvider?.();
    if (info) {
      this.sendPayloadEvent('client_bye', info.clientId, undefined);
    }
  }

  private startHello(): void {
    this.sendHello();
    this.stopHello();
    this.helloTimer = setInterval(() => this.sendHello(), HELLO_INTERVAL_MS);
  }

  private stopHello(): void {
    if (this.helloTimer !== -1) {
      clearInterval(this.helloTimer);
      this.helloTimer = -1;
    }
  }

  private sendHello(): void {
    const info = this.deviceInfoProvider?.();
    if (info) {
      // client_hello carries the full DeviceInfo; sealAndSend handles the JSON.
      void this.sealAndSend(helloPayload(info));
    }
  }

  private open(): void {
    if (this.closed) {
      return;
    }
    // Keys must exist before we auth: the `auth` frame carries channelToken.
    this.ensureKeys().then(() => {
      if (this.closed) {
        return;
      }
      const ws = webSocket.createWebSocket();
      this.ws = ws;
      this.authed = false;
      this.attemptSeq += 1;
      const seq = this.attemptSeq;
      // Diagnostic (2026-07-16): log which network bearer(s) this socket will use
      // to settle whether the atomic service's traffic rides an active VPN or
      // bypasses it (裸连). The browser (mobile-web) has no equivalent probe —
      // interpret by comparing relay connect latency against a browser on the
      // same phone. `*` marks the default net the socket actually binds to.
      this.logNetBearer(seq);
      // Watchdog: if this attempt neither auths nor dies within the window,
      // tear it down and retry (see CONNECT_ATTEMPT_TIMEOUT_MS above).
      setTimeout(() => {
        if (!this.closed && seq === this.attemptSeq && !this.authed) {
          this.forceReconnect('connect attempt timed out');
        }
      }, CONNECT_ATTEMPT_TIMEOUT_MS);

      ws.on('open', (_err: BusinessError, _value: Object) => {
        const token = this.channelToken ?? '';
        const auth: AuthFrame = { type: 'auth', role: 'client', secret: token };
        ws.send(JSON.stringify(auth), (_e: BusinessError, _ok: boolean) => {});
      });

      ws.on('message', (_err: BusinessError, value: string | ArrayBuffer) => {
        if (this.ws !== ws) {
          return; // stale socket already replaced
        }
        if (typeof value !== 'string') {
          return;
        }
        let frame: InboundFrame;
        try {
          frame = JSON.parse(value) as InboundFrame;
        } catch (e) {
          return;
        }
        this.handleFrame(frame);
      });

      ws.on('close', (_err: BusinessError, _value: Object) => {
        if (this.ws !== ws) {
          return; // stale socket already replaced
        }
        this.onSocketClosed();
      });

      ws.on('error', (err: BusinessError) => {
        if (this.ws !== ws) {
          return; // stale socket already replaced
        }
        // OHOS may deliver only 'error' (no 'close') for a failed socket, so
        // closing and waiting for the close event is not enough — reconnect.
        hilog.warn(DOMAIN, TAG, 'ws error: %{public}s', JSON.stringify(err));
        this.forceReconnect('socket error');
      });

      ws.connect(toWsUrl(this.relayUrl), (err: BusinessError, _ok: boolean) => {
        if (err) {
          hilog.warn(DOMAIN, TAG, 'ws connect failed: %{public}s', JSON.stringify(err));
          this.forceReconnect('connect failed');
        }
      });
    });
  }

  private onSocketClosed(): void {
    const wasAuthed = this.authed;
    this.teardownSocket();
    this.scheduleReconnect(wasAuthed);
  }

  /// Diagnostic probe: log the bearer type(s) of the DEFAULT net this socket
  /// binds to (WIFI / CELLULAR / VPN / …). `getAllNets` and the `BEARER_VPN`
  /// symbol are both @atomicservice-restricted (build error 11706010), so we can
  /// only inspect the default net and must recognise a VPN bearer by its numeric
  /// value (4). The restriction is itself telling: an atomic service is denied
  /// the API to even enumerate a system VPN's nets. If a VPN is active on the
  /// phone yet this logs WIFI/CELLULAR (never VPN), the atomic service's traffic
  /// is bypassing the tunnel — which is what the mobile-web browser gets for free.
  private logNetBearer(seq: number): void {
    connection.getDefaultNet().then((def: connection.NetHandle) => {
      connection.getNetCapabilities(def).then((cap: connection.NetCapabilities) => {
        const types: connection.NetBearType[] = cap.bearerTypes ?? [];
        const bearers = types.map((b: connection.NetBearType) => bearerName(b)).join('+');
        hilog.info(DOMAIN, TAG, 'net probe #%{public}d: default netId=%{public}d bearers=[%{public}s]',
          seq, def.netId, bearers.length > 0 ? bearers : '?');
      }).catch((e: Object) => {
        hilog.warn(DOMAIN, TAG, 'net probe caps failed: %{public}s', JSON.stringify(e));
      });
    }).catch((e: Object) => {
      hilog.warn(DOMAIN, TAG, 'net probe getDefaultNet failed: %{public}s', JSON.stringify(e));
    });
  }

  private handleFrame(frame: InboundFrame): void {
    switch (frame.type) {
      case 'authed':
        this.authed = true;
        this.reconnectDelay = 1000;
        this.handlers.onStatus?.(true);
        this.handlers.onAgentOnline?.(frame.agent_online === true);
        this.startHello();
        break;
      case 'agent_status':
        this.handlers.onAgentOnline?.(frame.online === true);
        break;
      case 'error':
        this.handlers.onAuthError?.(frame.message ?? '认证失败');
        break;
      case 'msg':
        if (frame.payload) {
          this.dispatchMsgPayload(frame.payload);
        }
        break;
      default:
        break; // notify frames are delivered out-of-band via Push Kit
    }
  }

  /// A `msg` payload is a sealed `{enc:"box",iv,ct}` envelope. Open it, then
  /// dispatch the JSON business payload. Anything not a well-formed sealed
  /// envelope from our paired desktop is dropped (the next snapshot self-heals).
  private dispatchMsgPayload(payload: SealedPayload): void {
    const iv = payload.iv;
    const ct = payload.ct;
    if (!isSealed(payload.enc, iv, ct) || iv === undefined || ct === undefined) {
      return;
    }
    const ek = this.encKey;
    if (!ek) {
      return;
    }
    const sealed: SealedBox = { enc: 'box', iv, ct };
    // z:true marks a payload gzipped before sealing (we advertise supportsGzip
    // once Gzip.gzipSelfTest passes) — open to bytes, inflate, then parse.
    const jsonPromise: Promise<string> = payload.z === true
      ? openBytes(ek, sealed)
        .then((bytes: Uint8Array) => gunzip(bytes))
        .then((plain: Uint8Array) => util.TextDecoder.create('utf-8').decodeToString(plain))
      : openString(ek, sealed);
    jsonPromise
      .then((json: string) => {
        let biz: BusinessPayload;
        try {
          biz = JSON.parse(json) as BusinessPayload;
        } catch (e) {
          return;
        }
        this.handlePayload(biz);
      })
      .catch((e: Object) => {
        // bad key / tampered / malformed / inflate failure — drop the frame
        if (payload.z === true) {
          hilog.warn(DOMAIN, TAG, 'gzipped frame dropped: %{public}s', JSON.stringify(e));
        }
      });
  }

  private handlePayload(payload: BusinessPayload): void {
    switch (payload.event) {
      case 'decision_created':
        if (payload.request !== undefined) {
          this.handlers.onDecisionCreated?.(payload.kind ?? '', payload.request);
        }
        break;
      case 'decision_resolved':
        this.handlers.onDecisionResolved?.(payload.kind ?? '', payload.id ?? '');
        break;
      case 'sessions':
        this.handlers.onSessions?.(payload.sessions ?? []);
        break;
      case 'ack': {
        // 早 ack(方案 A):桌面收到写 req 并开始处理。触发 onAck 一次;promise 仍
        // 待最终 reply / 超时才 settle。
        const reqId = payload.req_id ?? '';
        const entry = this.pending.get(reqId);
        if (entry && entry.onAck && entry.acked !== true) {
          entry.acked = true;
          entry.onAck();
        }
        break;
      }
      case 'reply': {
        const reqId = payload.req_id ?? '';
        const entry = this.pending.get(reqId);
        if (!entry) {
          return;
        }
        this.pending.delete(reqId);
        clearTimeout(entry.timer);
        if (payload.ok === true) {
          entry.resolve(payload.data ?? new EmptyObj());
        } else {
          entry.reject(new RelayRequestError(payload.error ?? '请求失败', true));
        }
        break;
      }
      default:
        break;
    }
  }

  private sendRaw(frame: Object): boolean {
    const ws = this.ws;
    if (!ws) {
      return false;
    }
    ws.send(JSON.stringify(frame), (_e: BusinessError, _ok: boolean) => {});
    return true;
  }

  /// Seal a business payload (a JSON string) and send it as a `msg` frame.
  private async sealAndSend(payloadJson: string): Promise<void> {
    const ws = this.ws;
    if (!ws || !this.authed) {
      return; // pre-auth frames would be a relay protocol violation
    }
    await this.ensureKeys();
    const ek = this.encKey;
    if (!ek) {
      return;
    }
    const sealed = await sealString(ek, payloadJson);
    const frame: MsgFrame = { type: 'msg', payload: sealed };
    ws.send(JSON.stringify(frame), (e: BusinessError, _ok: boolean) => {
      if (e && this.ws === ws) {
        this.forceReconnect('send failed');
      }
    });
  }

  /// Fire-and-forget seal+send for a `{event, clientId}` payload (bye).
  private sendPayloadEvent(event: string, clientId: string, _unused?: Object): void {
    const byeFrame: ByePayload = { event, clientId };
    void this.sealAndSend(JSON.stringify(byeFrame));
  }

  /// Answer a decision — mirrors mobile_relay::handle_answer's shapes and the web
  /// relay's `answer`/`sendPayload`. `payload` is a fully-formed answer envelope
  /// (a declared interface instance carrying {event:'answer', kind, id, …fields}).
  /// Kept as `Object` because ArkTS bans dynamic `Record` literals, so the merge
  /// happens on the caller side against a typed interface.
  ///
  /// Returns whether the socket was authed+open at call time — the sealed send
  /// itself completes on a later microtask (sealing is async). Callers use this
  /// to gate optimistic card removal: a `false` return means the frame was NOT
  /// sent (channel down), so the card must stay put instead of vanishing into a
  /// silent drop. If the send is lost after this point, the desktop's next
  /// pending_snapshot reconcile brings the card back (see MainShell). This is the
  /// exact contract the web client relies on (`if (client.answer(...)) …`).
  answer(payload: Object): boolean {
    if (!this.ws || !this.authed) {
      return false;
    }
    void this.sealAndSend(JSON.stringify(payload));
    return true;
  }

  /// Data request to the agent (pending_snapshot / task_plans / …). `params` is
  /// a declared interface instance (never a `Record` literal — banned in ArkTS).
  request<T>(method: string, params?: Object, timeoutMs?: number, onAck?: () => void): Promise<T> {
    this.reqSeq += 1;
    const reqId = `${this.reqPrefix}-${this.reqSeq}`;
    return new Promise<T>((resolve: (v: T) => void, reject: (e: Object) => void) => {
      if (!this.ws || !this.authed) {
        reject(new RelayRequestError('尚未连接 relay', false));
        return;
      }
      const timer = setTimeout(() => {
        this.pending.delete(reqId);
        reject(new RelayRequestError('请求超时（桌面端可能离线）', false));
      }, timeoutMs ?? REQUEST_TIMEOUT_MS);
      const entry: PendingEntry = {
        resolve: (v: Object) => resolve(v as T),
        reject,
        timer,
        onAck,
      };
      this.pending.set(reqId, entry);
      const reqFrame: ReqPayload = { event: 'req', req_id: reqId, method, params: params ?? new EmptyObj() };
      this.sealAndSend(JSON.stringify(reqFrame)).catch((_e: Object) => {
        const pendingEntry = this.pending.get(reqId);
        if (!pendingEntry) {
          return;
        }
        this.pending.delete(reqId);
        clearTimeout(pendingEntry.timer);
        pendingEntry.reject(new RelayRequestError('尚未连接 relay', false));
      });
    });
  }

  private failPending(message: string): void {
    this.pending.forEach((entry: PendingEntry) => {
      clearTimeout(entry.timer);
      entry.reject(new RelayRequestError(message, false));
    });
    this.pending.clear();
  }
}

// ── Payload shapes for outbound frames (typed literals for ArkTS) ──────────────

interface HelloPayload {
  event: string;
  clientId: string;
  label: string;
  platform: string;
  pushSubscribed: boolean;
  supportsGzip: boolean;
  supportsBinary: boolean;
}

interface ByePayload {
  event: string;
  clientId: string;
}

interface ReqPayload {
  event: string;
  req_id: string;
  method: string;
  params: Object;
}

/// Human-readable name for a NetBearType, for the network diagnostic probe.
function bearerName(b: connection.NetBearType): string {
  switch (b) {
    case connection.NetBearType.BEARER_CELLULAR:
      return 'CELLULAR';
    case connection.NetBearType.BEARER_WIFI:
      return 'WIFI';
    case connection.NetBearType.BEARER_ETHERNET:
      return 'ETHERNET';
    default:
      // BEARER_VPN (4) is an @atomicservice-restricted symbol, so match its
      // numeric value instead of the enum name to still label a VPN bearer.
      return (b as number) === 4 ? 'VPN' : `#${b}`;
  }
}

function helloPayload(info: DeviceInfo): string {
  const p: HelloPayload = {
    event: 'client_hello',
    clientId: info.clientId,
    label: info.label,
    platform: info.platform,
    pushSubscribed: info.pushSubscribed,
    supportsGzip: info.supportsGzip,
    supportsBinary: info.supportsBinary,
  };
  return JSON.stringify(p);
}
