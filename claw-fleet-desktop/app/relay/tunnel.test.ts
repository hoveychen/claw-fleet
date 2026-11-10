// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { RelayError, type RelayRequester } from "./relaySocket";
import {
  SW_TUNNEL_MESSAGE,
  decodeTunnelReply,
  encodeTunnelRequest,
  handleSwMessage,
  relayMountPath,
  serveTunnelRequest,
  toResponse,
  tunnelPrefix,
  type HttpParams,
  type TunnelResponse,
} from "./tunnel";

const PREFIX = "/desktop/h";
const ORIGIN = "https://relay.example";

function fakeRelay(answer: (params: HttpParams) => unknown): RelayRequester & { calls: HttpParams[] } {
  const calls: HttpParams[] = [];
  return {
    calls,
    async request<T>(method: string, params: Record<string, unknown>): Promise<T> {
      expect(method).toBe("http");
      calls.push(params as unknown as HttpParams);
      const out = answer(params as unknown as HttpParams);
      if (out instanceof Error) throw out;
      return out as T;
    },
  };
}

const text = (b: ArrayBuffer) => new TextDecoder().decode(b);

describe("relay mount detection", () => {
  it("detects only the /desktop/ mount", () => {
    expect(relayMountPath("/desktop/")).toBe("/desktop/");
    expect(relayMountPath("/desktop")).toBe("/desktop/");
    expect(relayMountPath("/desktop/index.html")).toBe("/desktop/");
    expect(relayMountPath("/")).toBeNull();
    expect(relayMountPath("/m/")).toBeNull();
    expect(relayMountPath("/desktopx/")).toBeNull();
    expect(tunnelPrefix("/desktop/")).toBe("/desktop/h");
  });
});

describe("encodeTunnelRequest", () => {
  it("strips the prefix, keeps the path encoded and decodes the query", () => {
    const p = encodeTunnelRequest(
      {
        method: "get",
        url: `${ORIGIN}/desktop/h/decision_asset/a%20b/x.png?session=s1&path=%2FUsers%2Fme%2FMy%20Project`,
        contentType: null,
        body: null,
      },
      PREFIX,
    );
    expect(p).toEqual({
      method: "GET",
      path: "/decision_asset/a%20b/x.png",
      query: { session: "s1", path: "/Users/me/My Project" },
      body: null,
      bodyB64: null,
      contentType: null,
    });
  });

  it("sends a JSON body as JSON and anything else as base64", () => {
    const json = encodeTunnelRequest(
      {
        method: "POST",
        url: `${ORIGIN}/desktop/h/spawn`,
        contentType: "application/json",
        body: new TextEncoder().encode('{"a":1}').buffer as ArrayBuffer,
      },
      PREFIX,
    );
    expect(json.body).toEqual({ a: 1 });
    expect(json.bodyB64).toBeNull();
    expect(json.query).toBeNull();

    const bin = encodeTunnelRequest(
      {
        method: "POST",
        url: `${ORIGIN}/desktop/h/elicitation/upload?name=f.bin`,
        contentType: "application/octet-stream",
        body: new Uint8Array([0, 255, 1]).buffer,
      },
      PREFIX,
    );
    expect(bin.body).toBeNull();
    expect(bin.bodyB64).toBe(btoa(String.fromCharCode(0, 255, 1)));
  });

  it("refuses a URL outside the prefix", () => {
    expect(() =>
      encodeTunnelRequest({ method: "GET", url: `${ORIGIN}/health`, contentType: null, body: null }, PREFIX),
    ).toThrow(/not under the tunnel prefix/);
  });
});

describe("decodeTunnelReply", () => {
  it("decodes text and base64 bodies", () => {
    const t = decodeTunnelReply({ status: 200, contentType: "application/json", body: '{"ok":1}' });
    expect(t.status).toBe(200);
    expect(text(t.body)).toBe('{"ok":1}');
    const b = decodeTunnelReply({ status: 200, contentType: "image/png", bodyB64: btoa("\x89PNG") });
    expect(Array.from(new Uint8Array(b.body))).toEqual([0x89, 0x50, 0x4e, 0x47]);
  });

  it("treats a reply with no status as a bad gateway", () => {
    expect(decodeTunnelReply(null).status).toBe(502);
  });
});

describe("serveTunnelRequest", () => {
  const req = { method: "GET", url: `${ORIGIN}/desktop/h/sessions_snapshot`, contentType: null, body: null };

  it("passes the Mac's 403 through for a non-whitelisted route", async () => {
    const relay = fakeRelay(() => ({ status: 403, contentType: "text/plain", body: "route not allowed" }));
    const r = await serveTunnelRequest(relay, req, PREFIX);
    expect(r.status).toBe(403);
    expect(text(r.body)).toBe("route not allowed");
  });

  it("maps a Mac without the http method to 501 and a lost frame to 504", async () => {
    const old = await serveTunnelRequest(
      fakeRelay(() => new RelayError("unknown method: http", true)),
      req,
      PREFIX,
    );
    expect(old.status).toBe(501);
    const lost = await serveTunnelRequest(
      fakeRelay(() => new RelayError("relay request timed out", false)),
      req,
      PREFIX,
    );
    expect(lost.status).toBe(504);
  });

  it("builds a Response that liveProxy can read", async () => {
    const r = await serveTunnelRequest(
      fakeRelay(() => ({ status: 200, contentType: "application/json", body: "[1,2]" })),
      req,
      PREFIX,
    );
    const resp = toResponse(r);
    expect(resp.ok).toBe(true);
    expect(resp.headers.get("Content-Type")).toBe("application/json");
    expect(await resp.json()).toEqual([1, 2]);
    expect(toResponse({ status: 204, contentType: null, body: new ArrayBuffer(0) }).status).toBe(204);
  });
});

describe("SW → page message protocol", () => {
  it("answers a tunnel request on the reply port", async () => {
    const relay = fakeRelay((p) => ({ status: 200, contentType: "text/plain", body: `you asked ${p.path}` }));
    const posted: TunnelResponse[] = [];
    const port = { postMessage: vi.fn((m: TunnelResponse) => posted.push(m)) } as unknown as MessagePort;
    const handled = await handleSwMessage(
      relay,
      { type: SW_TUNNEL_MESSAGE, method: "GET", url: `${ORIGIN}/desktop/h/wiki_asset/x`, contentType: null, body: null },
      port,
      PREFIX,
    );
    expect(handled).toBe(true);
    expect(posted).toHaveLength(1);
    expect(posted[0].status).toBe(200);
    expect(text(posted[0].body)).toBe("you asked /wiki_asset/x");
  });

  it("ignores other messages", async () => {
    const relay = fakeRelay(() => ({}));
    const port = { postMessage: vi.fn() } as unknown as MessagePort;
    expect(await handleSwMessage(relay, { type: "something-else" }, port, PREFIX)).toBe(false);
    expect(await handleSwMessage(relay, { type: SW_TUNNEL_MESSAGE }, undefined, PREFIX)).toBe(false);
    expect(relay.calls).toHaveLength(0);
  });
});
