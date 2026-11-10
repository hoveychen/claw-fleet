/**
 * HTTP-over-relay tunnel: the page side.
 *
 * When this bundle is served from a relay origin under `/desktop/`, the data
 * routes `fleet webui` would answer same-origin are instead reached through the
 * E2E-encrypted relay channel. Every such route lives under one URL prefix
 * (`/desktop/h/<route>`), and two things carry requests there:
 *
 *  - `fetch()` from page code (liveProxy and the few components that fetch an
 *    asset URL) is answered *directly* by `installTunnelFetch`, without the
 *    Service Worker — so the data plane does not depend on the page being
 *    SW-controlled (a hard reload bypasses the SW).
 *  - Requests the browser makes on its own (`<img src>`, `<iframe src>`,
 *    `<a download>`, `<video>`) can only be caught by the Service Worker
 *    (`public/sw.js`). The SW lives too briefly to hold a socket, so it posts
 *    each request to a window that holds the relay, which answers through
 *    `serveTunnelRequest` — the same function `installTunnelFetch` uses.
 *
 * Wire contract with the Mac (`request("http", params)`):
 *   params `{ method, path, query, body, bodyB64, contentType }`
 *   reply  `{ status, contentType, body, bodyB64 }`
 * `path` is the URL path below the prefix, still percent-encoded as it
 * appeared in the URL; `query` is decoded key → value (last one wins).
 */

import { RelayError, type RelayRequester } from "./relaySocket";

/** Where the desktop bundle is mounted on a relay origin. */
export const RELAY_MOUNT = "/desktop/";

/** Message type the SW posts to a window (see `public/sw.js`). */
export const SW_TUNNEL_MESSAGE = "fleet-tunnel-req";

/**
 * The relay mount this page was served under, or `null` for every other host
 * (`fleet webui` at `/`, the cloud container, Tauri). The path is the signal:
 * nothing else serves this bundle below `/desktop/`.
 */
export function relayMountPath(pathname: string): string | null {
  if (pathname === "/desktop" || pathname.startsWith(RELAY_MOUNT)) return RELAY_MOUNT;
  return null;
}

/** `/desktop/` → `/desktop/h` — the probe base liveProxy prepends to routes. */
export function tunnelPrefix(mount: string): string {
  return `${mount.replace(/\/$/, "")}/h`;
}

/** A request as the SW (or a `fetch` call) hands it over. */
export interface TunnelRequest {
  method: string;
  /** Absolute URL, under the tunnel prefix. */
  url: string;
  contentType: string | null;
  body: ArrayBuffer | null;
}

/** What goes back to the SW / becomes a `Response`. */
export interface TunnelResponse {
  status: number;
  contentType: string | null;
  body: ArrayBuffer;
}

export interface HttpParams {
  method: string;
  path: string;
  query: Record<string, string> | null;
  body: unknown;
  bodyB64: string | null;
  contentType: string | null;
}

export interface HttpReply {
  status?: number;
  contentType?: string | null;
  body?: string | null;
  bodyB64?: string | null;
}

function bytesToB64(bytes: Uint8Array): string {
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}

function b64ToBytes(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

/** Build the `http` method's params from a tunnelled request. */
export function encodeTunnelRequest(req: TunnelRequest, prefix: string): HttpParams {
  const url = new URL(req.url);
  if (!url.pathname.startsWith(`${prefix}/`)) {
    throw new Error(`${url.pathname} is not under the tunnel prefix ${prefix}`);
  }
  const path = url.pathname.slice(prefix.length);
  let query: Record<string, string> | null = null;
  for (const [k, v] of url.searchParams) {
    (query ??= {})[k] = v;
  }
  let body: unknown = null;
  let bodyB64: string | null = null;
  const bytes = req.body ? new Uint8Array(req.body) : null;
  if (bytes && bytes.length > 0) {
    // JSON travels as JSON so the Mac can re-serialize it; anything else
    // (octet-stream uploads, a JSON body that does not parse) as base64.
    let parsed = false;
    if (req.contentType && /\bjson\b/i.test(req.contentType)) {
      try {
        body = JSON.parse(new TextDecoder().decode(bytes));
        parsed = true;
      } catch {
        /* fall through to base64 */
      }
    }
    if (!parsed) bodyB64 = bytesToB64(bytes);
  }
  return {
    method: req.method.toUpperCase(),
    path,
    query,
    body,
    bodyB64,
    contentType: req.contentType,
  };
}

/** Turn the Mac's reply into bytes plus status. */
export function decodeTunnelReply(reply: HttpReply | null | undefined): TunnelResponse {
  const status = typeof reply?.status === "number" ? reply.status : 502;
  const contentType = reply?.contentType ?? null;
  let bytes: Uint8Array;
  if (typeof reply?.bodyB64 === "string") bytes = b64ToBytes(reply.bodyB64);
  else if (typeof reply?.body === "string") bytes = new TextEncoder().encode(reply.body);
  else bytes = new Uint8Array(0);
  return { status, contentType, body: toArrayBuffer(bytes) };
}

function errorResponse(status: number, message: string): TunnelResponse {
  return {
    status,
    contentType: "text/plain; charset=utf-8",
    body: toArrayBuffer(new TextEncoder().encode(message)),
  };
}

/**
 * Carry one request over the relay and return what the browser should see.
 *
 * Never throws: a transport failure becomes an HTTP-shaped error so callers
 * (liveProxy's `!resp.ok` branch, an `<img>`'s onerror) degrade the way they
 * already do for a server error, instead of hanging.
 */
export async function serveTunnelRequest(
  relay: RelayRequester,
  req: TunnelRequest,
  prefix: string,
  timeoutMs?: number,
): Promise<TunnelResponse> {
  let params: HttpParams;
  try {
    params = encodeTunnelRequest(req, prefix);
  } catch (e) {
    return errorResponse(400, String(e instanceof Error ? e.message : e));
  }
  try {
    const reply = await relay.request<HttpReply>(
      "http",
      params as unknown as Record<string, unknown>,
      timeoutMs,
    );
    return decodeTunnelReply(reply);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    // A Mac that predates the `http` method answers "unknown method".
    if (e instanceof RelayError && e.remote) {
      return errorResponse(/unknown method/i.test(msg) ? 501 : 502, msg);
    }
    return errorResponse(504, msg);
  }
}

/** Statuses whose `Response` must not carry a body. */
const NULL_BODY_STATUS = new Set([101, 204, 205, 304]);

export function toResponse(r: TunnelResponse): Response {
  const status = r.status >= 200 && r.status <= 599 ? r.status : 502;
  const headers: Record<string, string> = {};
  if (r.contentType) headers["Content-Type"] = r.contentType;
  return new Response(NULL_BODY_STATUS.has(status) ? null : r.body, { status, headers });
}

/** Whether `input` targets the tunnel on this origin. */
function tunnelUrlOf(input: RequestInfo | URL, prefix: string): URL | null {
  const raw = input instanceof Request ? input.url : String(input);
  let url: URL;
  try {
    url = new URL(raw, window.location.href);
  } catch {
    return null;
  }
  if (url.origin !== window.location.origin) return null;
  return url.pathname.startsWith(`${prefix}/`) ? url : null;
}

/**
 * Replace `window.fetch` so tunnel URLs are answered over the relay from this
 * page. Every other URL goes to the original `fetch` untouched.
 */
export function installTunnelFetch(relay: RelayRequester, prefix: string): void {
  const original = window.fetch.bind(window);
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = tunnelUrlOf(input, prefix);
    if (!url) return original(input, init);
    const request = new Request(input instanceof Request ? input : url.toString(), init);
    const body =
      request.method === "GET" || request.method === "HEAD" ? null : await request.arrayBuffer();
    const r = await serveTunnelRequest(
      relay,
      {
        method: request.method,
        url: url.toString(),
        contentType: request.headers.get("Content-Type"),
        body: body && body.byteLength > 0 ? body : null,
      },
      prefix,
    );
    return toResponse(r);
  };
}

/** The SW → page message (see `public/sw.js`). */
export interface SwTunnelMessage extends TunnelRequest {
  type: typeof SW_TUNNEL_MESSAGE;
}

/**
 * Answer one SW message on its reply port. Returns `false` for messages that
 * are not tunnel requests, so the caller can ignore them.
 */
export async function handleSwMessage(
  relay: RelayRequester,
  data: unknown,
  port: MessagePort | undefined,
  prefix: string,
): Promise<boolean> {
  const msg = data as Partial<SwTunnelMessage> | null;
  if (!msg || msg.type !== SW_TUNNEL_MESSAGE || !port) return false;
  const r = await serveTunnelRequest(
    relay,
    {
      method: String(msg.method ?? "GET"),
      url: String(msg.url ?? ""),
      contentType: msg.contentType ?? null,
      body: msg.body ?? null,
    },
    prefix,
  );
  port.postMessage(r, [r.body]);
  return true;
}

/** Listen for the SW's tunnel requests. Call before the SW can send any. */
export function attachSwBridge(relay: RelayRequester, prefix: string): void {
  if (!("serviceWorker" in navigator)) return;
  navigator.serviceWorker.addEventListener("message", (ev) => {
    void handleSwMessage(relay, ev.data, ev.ports[0], prefix);
  });
  // Messages to a page are queued until it opts in; `addEventListener` alone
  // does not, unlike assigning `onmessage`.
  navigator.serviceWorker.startMessages();
}
