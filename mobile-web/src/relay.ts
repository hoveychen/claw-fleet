// WebSocket client for fleet-relay. Speaks the relay envelope
// (auth/msg/notify/presence/agent_status) and the Fleet business frames
// inside msg.payload (decision_created / decision_resolved / sessions /
// answer / req / reply) — see claw-fleet-core/src/mobile_relay.rs.

import { t } from "./i18n";
import type { DecisionKind, SessionInfo } from "./types";

/** Self-description this phone announces to the desktop so it appears in the
 *  desktop 「移动端」 device list. Provided lazily so `pushSubscribed` reflects
 *  the state at each heartbeat, not just at construction. */
export interface DeviceInfo {
  clientId: string;
  label: string;
  platform: string;
  pushSubscribed: boolean;
  /** Whether this browser can inflate a gzipped `sessions` snapshot via the
   *  native `DecompressionStream`. The desktop only compresses a snapshot when
   *  every live client reports `true`, so an old client here (or one on a
   *  browser without the API) transparently keeps receiving plaintext. */
  supportsGzip: boolean;
}

/** Native gzip inflation support (Safari 16.4+, all evergreen Chrome/Firefox).
 *  Announced to the desktop in `client_hello` so it can gate compression. */
export function gzipSupported(): boolean {
  return typeof DecompressionStream !== "undefined";
}

/** Inflate a base64-encoded gzip blob (the `enc:"gzip"` sessions payload) back
 *  into its JSON string, using the streaming DecompressionStream API. */
async function inflateGzipBase64(b64: string): Promise<string> {
  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
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
  private reqSeq = 0;
  // Per-instance prefix so req_ids never collide across devices sharing a
  // channel: the relay broadcasts every reply to all clients (registry.rs
  // forward), so a bare counter (r1, r2…) would let one phone's reply match
  // another's identically-numbered pending. See relay.test.ts.
  private reqPrefix = crypto.randomUUID();
  private pending = new Map<
    string,
    { resolve: (v: unknown) => void; reject: (e: Error) => void; timer: number }
  >();
  private reconnectDelay = 1000;
  private closed = false;
  private authed = false;

  constructor(secret: string, handlers: RelayHandlers, deviceInfo?: () => DeviceInfo) {
    this.secret = secret;
    this.handlers = handlers;
    this.deviceInfo = deviceInfo;
  }

  connect() {
    this.closed = false;
    this.open();
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

  private open() {
    if (this.closed) return;
    const ws = new WebSocket(relayWsUrl());
    this.ws = ws;
    this.authed = false;
    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "auth", role: "client", secret: this.secret }));
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
      window.setTimeout(() => this.open(), delay);
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
        this.handlePayload((frame.payload ?? {}) as Record<string, unknown>);
        break;
      default:
        break; // notify frames are handled by the service worker via Web Push
    }
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
        const raw = payload.sessions;
        // Compressed frame (desktop confirmed every live client supports gzip).
        // Decode is async; a decode failure is dropped silently because the
        // next snapshot is full state and self-heals the list.
        if (payload.enc === "gzip" && typeof raw === "string") {
          void inflateGzipBase64(raw)
            .then((json) => this.handlers.onSessions?.(JSON.parse(json) as SessionInfo[]))
            .catch(() => {});
        } else {
          this.handlers.onSessions?.((raw ?? []) as SessionInfo[]);
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
          entry.reject(new Error(String(payload.error ?? t("请求失败"))));
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

  private sendPayload(payload: unknown): boolean {
    return this.sendRaw({ type: "msg", payload });
  }

  /** Answer a decision — mirrors mobile_relay::handle_answer's shapes. */
  answer(kind: DecisionKind, id: string, fields: Record<string, unknown>): boolean {
    return this.sendPayload({ event: "answer", kind, id, ...fields });
  }

  /** Data request to the agent (pending_snapshot / task_plans / …).
   *  `timeoutMs` overrides the default for slow methods (LLM analysis). */
  request<T>(method: string, params?: Record<string, unknown>, timeoutMs?: number): Promise<T> {
    const reqId = `${this.reqPrefix}-${++this.reqSeq}`;
    return new Promise<T>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending.delete(reqId);
        reject(new Error(t("请求超时（桌面端可能离线）")));
      }, timeoutMs ?? REQUEST_TIMEOUT_MS);
      this.pending.set(reqId, {
        resolve: resolve as (v: unknown) => void,
        reject,
        timer,
      });
      if (!this.sendPayload({ event: "req", req_id: reqId, method, params: params ?? {} })) {
        this.pending.delete(reqId);
        window.clearTimeout(timer);
        reject(new Error(t("尚未连接 relay")));
      }
    });
  }

  /** Register a Web Push subscription on this channel. */
  pushSubscribe(subscription: unknown): boolean {
    return this.sendRaw({ type: "push_subscribe", subscription });
  }

  private failPending(message: string) {
    for (const [, entry] of this.pending) {
      window.clearTimeout(entry.timer);
      entry.reject(new Error(message));
    }
    this.pending.clear();
  }
}
