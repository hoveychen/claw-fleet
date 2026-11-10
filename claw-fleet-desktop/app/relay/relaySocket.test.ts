// Round-trips a request through RelaySocket against a fake relay whose "Mac"
// opens and seals with the shared crypto — the same keys the real desktop
// derives from the pairing secret.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { deriveKeys, open, seal } from "../../../shared-ts/relayCrypto";
import { RelayError, RelaySocket, relayWsUrl } from "./relaySocket";

const SECRET = "test-pairing-secret";

class FakeWs {
  static OPEN = 1;
  static last: FakeWs | null = null;
  readyState = 0;
  url: string;
  sent: Record<string, unknown>[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  /** Mac-side behaviour for each sealed `req`. */
  static mac: (req: Record<string, unknown>) => Record<string, unknown> | null = () => null;

  constructor(url: string) {
    this.url = url;
    FakeWs.last = this;
    queueMicrotask(() => {
      this.readyState = FakeWs.OPEN;
      this.onopen?.();
    });
  }

  send(data: string) {
    const frame = JSON.parse(data) as Record<string, unknown>;
    this.sent.push(frame);
    if (frame.type === "auth") {
      void deriveKeys(SECRET).then((k) => {
        expect(frame.secret).toBe(k.channelToken);
        this.deliver({ type: "authed", agent_online: true });
      });
    } else if (frame.type === "msg") {
      void (async () => {
        const { encKey } = await deriveKeys(SECRET);
        const req = JSON.parse(await open(encKey, frame.payload as never));
        const reply = FakeWs.mac(req);
        if (reply) this.deliver({ type: "msg", payload: await seal(encKey, JSON.stringify(reply)) });
      })();
    }
  }

  deliver(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }

  close() {
    this.readyState = 3;
    this.onclose?.();
  }
}

beforeEach(() => {
  vi.stubGlobal("WebSocket", FakeWs);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("RelaySocket", () => {
  it("builds the relay's ws URL", () => {
    expect(relayWsUrl("https://relay.example/")).toBe("wss://relay.example/ws");
  });

  it("auths with the channel token and resolves a sealed http reply", async () => {
    FakeWs.mac = (req) => ({
      event: "reply",
      req_id: req.req_id,
      ok: true,
      data: { status: 200, contentType: "application/json", body: `"${(req.params as { path: string }).path}"` },
    });
    const s = new RelaySocket(SECRET, "https://relay.example");
    s.connect();
    await s.waitAuthed(2000);
    const data = await s.request<{ status: number; body: string }>("http", { method: "GET", path: "/health" });
    expect(data).toEqual({ status: 200, contentType: "application/json", body: '"/health"' });
    const msg = FakeWs.last!.sent.find((f) => f.type === "msg")!;
    // The relay sees an ack id for custody reports, never the plaintext.
    expect(typeof msg.ack_id).toBe("string");
    expect(JSON.stringify(msg)).not.toContain("/health");
    s.close();
  });

  it("rejects with a remote verdict when the Mac answers ok:false", async () => {
    FakeWs.mac = (req) => ({ event: "reply", req_id: req.req_id, ok: false, error: "unknown method: http" });
    const s = new RelaySocket(SECRET, "https://relay.example");
    s.connect();
    await s.waitAuthed(2000);
    const err = await s.request("http", {}).catch((e) => e);
    expect(err).toBeInstanceOf(RelayError);
    expect((err as RelayError).remote).toBe(true);
    s.close();
  });

  it("fails fast when the relay drops the frame", async () => {
    FakeWs.mac = () => null;
    const s = new RelaySocket(SECRET, "https://relay.example");
    s.connect();
    await s.waitAuthed(2000);
    const p = s.request("http", {});
    await new Promise((r) => setTimeout(r, 20));
    const msg = FakeWs.last!.sent.find((f) => f.type === "msg")!;
    FakeWs.last!.deliver({ type: "msg_ack", ack_id: msg.ack_id, status: "dropped" });
    const err = await p.catch((e) => e);
    expect((err as RelayError).remote).toBe(false);
    s.close();
  });
});
