// WebSocket client for fleet-relay. Speaks the relay envelope
// (auth/msg/notify/presence/agent_status) and the Fleet business frames
// inside msg.payload (decision_created / decision_resolved / sessions /
// answer / req / reply) — see claw-fleet-core/src/mobile_relay.rs.

import type { DecisionKind, SessionInfo } from "./types";

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

export class RelayClient {
  private ws: WebSocket | null = null;
  private secret: string;
  private handlers: RelayHandlers;
  private reqSeq = 0;
  private pending = new Map<
    string,
    { resolve: (v: unknown) => void; reject: (e: Error) => void; timer: number }
  >();
  private reconnectDelay = 1000;
  private closed = false;
  private authed = false;

  constructor(secret: string, handlers: RelayHandlers) {
    this.secret = secret;
    this.handlers = handlers;
  }

  connect() {
    this.closed = false;
    this.open();
  }

  close() {
    this.closed = true;
    this.ws?.close();
    this.ws = null;
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
      this.handlers.onStatus?.(false);
      this.failPending("连接已断开");
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
        break;
      case "agent_status":
        this.handlers.onAgentOnline?.(Boolean(frame.online));
        break;
      case "error":
        this.handlers.onAuthError?.(String(frame.message ?? "认证失败"));
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
        this.handlers.onDecisionResolved?.(
          payload.kind as DecisionKind,
          String(payload.id ?? ""),
        );
        break;
      case "sessions":
        this.handlers.onSessions?.((payload.sessions ?? []) as SessionInfo[]);
        break;
      case "reply": {
        const reqId = String(payload.req_id ?? "");
        const entry = this.pending.get(reqId);
        if (!entry) return;
        this.pending.delete(reqId);
        window.clearTimeout(entry.timer);
        if (payload.ok) {
          entry.resolve(payload.data);
        } else {
          entry.reject(new Error(String(payload.error ?? "请求失败")));
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

  /** Data request to the agent (pending_snapshot / task_plans / …). */
  request<T>(method: string, params?: Record<string, unknown>): Promise<T> {
    const reqId = `r${++this.reqSeq}`;
    return new Promise<T>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending.delete(reqId);
        reject(new Error("请求超时（桌面端可能离线）"));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(reqId, {
        resolve: resolve as (v: unknown) => void,
        reject,
        timer,
      });
      if (!this.sendPayload({ event: "req", req_id: reqId, method, params: params ?? {} })) {
        this.pending.delete(reqId);
        window.clearTimeout(timer);
        reject(new Error("尚未连接 relay"));
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
