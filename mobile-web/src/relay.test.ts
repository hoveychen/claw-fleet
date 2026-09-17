import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { RelayClient, RelayRequestError, type RttSample, isDesktopRejection } from "./relay";
// URL membership calculation moved to relayBase.ts (a leaf module without relay client).
import { relayDisplayHost, resolveRelayBase } from "./relayBase";
import { deriveKeys, isSealed, open, type RelayKeys, seal, sealBytes } from "./relayCrypto";

// relay.ts depends on browser globals (window.setTimeout/WebSocket/location) which don't exist in node;
// we inject minimal shims and use a fake WebSocket to capture outgoing frames and manually deliver received frames.
//
// After end-to-end encryption, each `msg.payload` is a ciphertext envelope {enc:"box"} (auth/authed/error
// and other relay control frames remain plaintext). So: captured outbound frames must be decrypted with the
// shared keypair to verify; injected inbound business frames must be encrypted before delivery. Keys match
// on both ends (same secret derives same key), which is how relay broadcast cross-wiring works in the first
// place. Tests run in node where crypto.subtle is available globally.

const SECRET = "shared-secret-1234567890";
let KEYS: RelayKeys;
beforeAll(async () => {
  KEYS = await deriveKeys(SECRET);
});

class FakeWs {
  static OPEN = 1;
  static instances: FakeWs[] = [];
  readyState = FakeWs.OPEN;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];
  constructor(public url: string) {
    FakeWs.instances.push(this);
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {}
  /** Simulate relay delivering a text frame to this connection. */
  deliver(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}

/** Sleep for a fixed duration.
 *
 *  **Only use this in two cases — never use it to wait for something to happen** (that's `waitFor`'s job):
 *  1. Negative assertion — give "incorrect behavior" a real chance to occur before asserting it didn't
 *     (like `expect(settled).toBe(false)`). Polling would return immediately without verifying anything,
 *     making the test weaker.
 *  2. Time fixture — intentionally advance the clock so a specific duration itself is verifiable
 *     (e.g. assert `totalMs >= 40` after `tick(40)`). */
const tick = (ms = 20) => new Promise((r) => setTimeout(r, ms));

/** Wait until `ok()` returns true (up to ~2s), rather than sleeping for a fixed duration.
 *
 *  Fixed-duration `await tick()` only works for frames that "unseal once and callback". gzip frames must
 *  first unseal via WebCrypto, then pass through `DecompressionStream` inflate before calling back — this is
 *  the heaviest async work in a single tick in this file. If the machine slows by 20ms, we fail; assertions
 *  see the initial value before the callback runs. This rarely happens in warm vite cache (0/29) but can
 *  happen in cold cache (1/6). CI always cold-starts, so the odds are higher there. Raising the sleep just
 *  shifts the window; polling eliminates it — `nextWs` uses the same pattern. */
async function waitFor(ok: () => boolean | Promise<boolean>, what: string): Promise<void> {
  for (let i = 0; i < 200; i++) {
    if (await ok()) return;
    await tick(10);
  }
  throw new Error(`${what} 未在预期时间内发生`);
}

/** Connection is established asynchronously (open() awaits key derivation before new WebSocket),
 *  so wait until a new FakeWs appears and return it. */
async function nextWs(baseline: number): Promise<FakeWs> {
  for (let i = 0; i < 200; i++) {
    if (FakeWs.instances.length > baseline) return FakeWs.instances[FakeWs.instances.length - 1];
    await tick(1);
  }
  throw new Error("ws 未在预期时间内创建");
}

/** Seal a business payload into a ciphertext `msg` frame the desktop would emit (z=false, plaintext sealed directly). */
async function sealedMsg(payload: unknown): Promise<{ type: string; payload: unknown }> {
  return { type: "msg", payload: await seal(KEYS.encKey, JSON.stringify(payload)) };
}

/** Unseal all ciphertext `msg` business payloads sent from a connection. */
async function openSent(ws: FakeWs): Promise<Array<Record<string, unknown>>> {
  const out: Array<Record<string, unknown>> = [];
  for (const raw of ws.sent) {
    const f = JSON.parse(raw);
    if (f.type === "msg" && isSealed(f.payload)) {
      out.push(JSON.parse(await open(KEYS.encKey, f.payload)));
    }
  }
  return out;
}

/** Get the req_id of a `req` frame sent from a connection (unseal first).
 *
 *  Outbound encryption is asynchronous, so we wait until the frame actually lands in `ws.sent` rather than
 *  having the caller sleep a fixed duration and hope — if sleep is insufficient, throw "no req frame sent from this connection". */
async function sentReqId(ws: FakeWs): Promise<string> {
  let found: string | undefined;
  await waitFor(async () => {
    for (const p of await openSent(ws)) {
      if (p.event === "req") {
        found = String(p.req_id);
        return true;
      }
    }
    return false;
  }, "该连接发出 req 帧");
  return found!;
}

const windowShim = () => ({
  location: { origin: "http://localhost" },
  setTimeout: (fn: () => void, ms?: number) => setTimeout(fn, ms) as unknown as number,
  clearTimeout: (id: number) => clearTimeout(id),
  setInterval: (fn: () => void, ms?: number) => setInterval(fn, ms) as unknown as number,
  clearInterval: (id: number) => clearInterval(id),
});

/** Create an authed client and return it with its fake ws. */
async function connected(clients: RelayClient[]): Promise<{ client: RelayClient; ws: FakeWs }> {
  const base = FakeWs.instances.length;
  const client = new RelayClient(SECRET, {});
  clients.push(client);
  client.connect();
  const ws = await nextWs(base);
  ws.onopen?.();
  ws.deliver({ type: "authed", agent_online: true, clients: 1 });
  return { client, ws };
}

describe("RelayClient authenticates connections with channelToken", () => {
  const clients: RelayClient[] = [];
  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });
  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  it("auth frame sends derived channelToken, not raw secret", async () => {
    const { ws } = await connected(clients);
    const authFrame = ws.sent.map((s) => JSON.parse(s)).find((f) => f.type === "auth");
    expect(authFrame).toBeTruthy();
    expect(authFrame.secret).toBe(KEYS.channelToken);
    expect(authFrame.secret).not.toBe(SECRET);
    // channelToken is 64 hex (HKDF 256 bit); relay sees it as an opaque token.
    expect(authFrame.secret).toMatch(/^[0-9a-f]{64}$/);
  });
});

describe("RelayClient isolates req_id across devices", () => {
  const clients: RelayClient[] = [];

  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  afterEach(() => {
    // Clean up 15s timeout/heartbeat timers to avoid leaking into other tests.
    for (const c of clients.splice(0)) c.close();
  });

  // Core bug: two phones with the same secret land on the same relay channel. Relay broadcasts the agent's
  // reply frame to every client in that channel (registry.rs forward), but the frame carries no client routing.
  // If both phones share req_id space (both start from 1), A's reply gets matched by B's pending request with the
  // same number, and B parses A's data. Same secret derives same encKey, so B can also unseal the broadcast
  // ciphertext — routing only differentiates by reqPrefix (a UUID per instance).
  it("When A's reply is broadcast to B, B does not use it to resolve its own same-numbered request", async () => {
    const a = await connected(clients);
    const b = await connected(clients);

    const pa = a.client.request<{ who: string }>("tail", { session: "A" });
    const pb = b.client.request<{ who: string }>("pending_snapshot", {});

    let bSettled: unknown = "PENDING";
    pb.then((v) => (bSettled = v)).catch(() => (bSettled = "REJECTED"));

    const reqIdA = await sentReqId(a.ws);

    // Agent's reply to A's request (carrying A's req_id) is broadcast by relay to both A and B connections.
    const replyForA = await sealedMsg({
      event: "reply",
      req_id: reqIdA,
      ok: true,
      data: { who: "A-tail" },
    });
    a.ws.deliver(replyForA);
    b.ws.deliver(replyForA); // Broadcast leaks to B

    await expect(pa).resolves.toEqual({ who: "A-tail" });

    // Negative assertion: give microtasks/timers a chance to run, then assert B wasn't cross-wired by A's reply.
    // Must use fixed sleep here — polling "bSettled is still PENDING" returns immediately without verifying.
    await tick();
    expect(bSettled).toBe("PENDING");
  });
});

describe("RelayClient early ack (approach A)", () => {
  const clients: RelayClient[] = [];

  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  it("Receiving ack triggers onAck, but promise still waits for reply to resolve", async () => {
    const { client, ws } = await connected(clients);
    let acked = false;
    const p = client.request<{ ok: boolean }>("spawn_session", {}, undefined, () => {
      acked = true;
    });
    let settled: unknown = "PENDING";
    p.then((v) => (settled = v)).catch(() => (settled = "REJECTED"));
    const reqId = await sentReqId(ws);

    // Early ack arrives: onAck triggers, but promise doesn't resolve (still waiting for final reply).
    ws.deliver(await sealedMsg({ event: "ack", req_id: reqId }));
    // First wait for ack to actually be processed (positive), then sleep more to give "promise resolves
    // incorrectly" a chance — if we don't, and the machine is slow, neither event has happened yet and
    // the assertion below passes falsely.
    await waitFor(() => acked, "onAck 触发");
    await tick();
    expect(acked).toBe(true);
    expect(settled).toBe("PENDING");

    // Promise resolves only when the final reply arrives.
    ws.deliver(await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: { ok: true } }));
    await expect(p).resolves.toEqual({ ok: true });
  });

  it("Duplicate ack triggers onAck only once", async () => {
    const { client, ws } = await connected(clients);
    let count = 0;
    const p = client.request("spawn_session", {}, undefined, () => {
      count++;
    });
    p.catch(() => {});
    const reqId = await sentReqId(ws);
    ws.deliver(await sealedMsg({ event: "ack", req_id: reqId }));
    ws.deliver(await sealedMsg({ event: "ack", req_id: reqId }));
    // First wait for the first ack to land (positive), then sleep to give the second ack a chance to
    // incorrectly trigger twice. If we only sleep a fixed duration, on a slow machine neither may have run,
    // and `count === 1` becomes a false green.
    await waitFor(() => count >= 1, "首个 ack 触发 onAck");
    await tick();
    expect(count).toBe(1);
  });
});

// The full round trip (phone → relay → desktop → relay → phone) is five segments. Reporting only
// the total makes it impossible to diagnose where the slowdown is. Using two existing observation
// points: relay's immediate msg_ack response isolates the phone↔relay segment (excluding desktop),
// and desktop's handle_ms timestamp in the reply covers its own handler time. The remainder is the
// relay↔desktop segment. Missing observations must report null, never 0 — 0 falsely claims "that
// segment took zero", shifting the lost time to the residual.
describe("RelayClient RTT segmentation", () => {
  const clients: RelayClient[] = [];

  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  /** Make a request and return its req_id and the sample slot received. */
  async function requestWithSamples(): Promise<{
    ws: FakeWs;
    reqId: string;
    samples: RttSample[];
  }> {
    const base = FakeWs.instances.length;
    const samples: RttSample[] = [];
    const client = new RelayClient(SECRET, { onRttSample: (s) => samples.push(s) });
    clients.push(client);
    client.connect();
    const ws = await nextWs(base);
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });
    const p = client.request("today_usage");
    p.catch(() => {});
    return { ws, reqId: await sentReqId(ws), samples };
  }

  it("ack 与 handle_ms 都在时，三段可分辨", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    // Relay replies with msg_ack first (desktop not involved yet), then desktop's reply arrives later.
    // Time fixture: this 40ms is the quantity itself that the `totalMs >= 40` assertion below verifies; can't be replaced with polling.
    ws.deliver({ type: "msg_ack", ack_id: reqId, status: "delivered" });
    await tick(40);
    ws.deliver(
      await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: {}, handle_ms: 380 }),
    );
    await waitFor(() => samples.length >= 1, "RTT 样本产出");

    expect(samples).toHaveLength(1);
    const s = samples[0];
    expect(s.desktopHandleMs).toBe(380);
    expect(s.phoneRelayMs).not.toBeNull();
    // ack arrives before reply, so phone leg must be less than total — if we reversed them, residual would go negative.
    expect(s.phoneRelayMs!).toBeLessThan(s.totalMs);
    expect(s.totalMs).toBeGreaterThanOrEqual(40);
  });

  it("When msg_ack doesn't arrive, phone segment reports null, not 0", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    ws.deliver(
      await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: {}, handle_ms: 12 }),
    );
    await waitFor(() => samples.length >= 1, "RTT 样本产出");
    expect(samples[0].phoneRelayMs).toBeNull();
    expect(samples[0].desktopHandleMs).toBe(12);
  });

  it("When old desktop lacks handle_ms, desktop segment reports null, not 0", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    ws.deliver({ type: "msg_ack", ack_id: reqId, status: "delivered" });
    await tick();
    ws.deliver(await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: {} }));
    await waitFor(() => samples.length >= 1, "RTT 样本产出");
    expect(samples[0].desktopHandleMs).toBeNull();
    expect(samples[0].phoneRelayMs).not.toBeNull();
  });

  it("Desktop rejection (ok:false) still produces a sample—slowness and failure are different things", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    ws.deliver({ type: "msg_ack", ack_id: reqId, status: "delivered" });
    await tick();
    ws.deliver(
      await sealedMsg({ event: "reply", req_id: reqId, ok: false, error: "nope", handle_ms: 7 }),
    );
    await waitFor(() => samples.length >= 1, "RTT 样本产出");
    expect(samples).toHaveLength(1);
    expect(samples[0].desktopHandleMs).toBe(7);
  });
});

// A request can fail for two completely different reasons, and the correct caller response is opposite:
//   - Desktop explicitly replied ok:false ("Workspace directory not found: ...") — desktop received,
//     judged, rejected. Retry/wait are pointless; error should go to user immediately.
//   - reply frame dropped (network switch/screen sleep/reconnect; relay is best-effort, no queue) — desktop
//     likely already acted; only the receipt is missing. This is when a grace period watching the snapshot makes sense.
// Before, both were bare Error and Composer couldn't tell them apart, so it dragged desktop's explicit
// rejection into a 20-second grace period, appearing as "clicked, no response, error after twenty seconds".
describe("RelayClient distinguishes failure sources", () => {
  const clients: RelayClient[] = [];

  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  it("Desktop reply with ok:false → remote error, carries desktop message", async () => {
    const { client, ws } = await connected(clients);
    const p = client.request("spawn_session", { workspacePath: "~/nope" });
    const reply = await sealedMsg({
      event: "reply",
      req_id: await sentReqId(ws),
      ok: false,
      error: "Workspace directory not found: /Users/x/nope",
    });
    ws.deliver(reply);
    const err = await p.catch((e) => e);
    expect(err).toBeInstanceOf(RelayRequestError);
    expect((err as RelayRequestError).remote).toBe(true);
    expect((err as Error).message).toContain("Workspace directory not found");
    expect(isDesktopRejection(err)).toBe(true);
  });

  it("Request timeout (frame may be lost) → non-remote error, caller can enter grace period", async () => {
    const { client } = await connected(clients);
    const p = client.request("spawn_session", {}, 5); // 5ms timeout, no reply delivered
    const err = await p.catch((e) => e);
    expect(err).toBeInstanceOf(RelayRequestError);
    expect((err as RelayRequestError).remote).toBe(false);
    expect(isDesktopRejection(err)).toBe(false);
  });

  it("Not connected → non-remote error", async () => {
    const client = new RelayClient(SECRET, {});
    clients.push(client);
    const err = await client.request("spawn_session", {}).catch((e) => e);
    expect(isDesktopRejection(err)).toBe(false);
  });

  it("Regular Error / non-Error values are not mistaken for desktop rejection", () => {
    expect(isDesktopRejection(new Error("boom"))).toBe(false);
    expect(isDesktopRejection("boom")).toBe(false);
    expect(isDesktopRejection(undefined)).toBe(false);
  });
});

/** Gzip a JSON string the same way the desktop does, return raw bytes (simulate desktop plaintext with z:true). */
async function gzipBytes(json: string): Promise<ArrayBuffer> {
  const stream = new Blob([new TextEncoder().encode(json)])
    .stream()
    .pipeThrough(new CompressionStream("gzip"));
  return new Response(stream).arrayBuffer();
}

describe("RelayClient sessions snapshot exchange (encrypted)", () => {
  const clients: RelayClient[] = [];
  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });
  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  it("Encrypted sessions frame is decrypted and passed through (z defaults to uncompressed)", async () => {
    const sessions = [
      { id: "s1", workspaceName: "alpha", status: "active" },
      { id: "s2", workspaceName: "beta", status: "idle" },
    ];
    let got: unknown = null;
    const base = FakeWs.instances.length;
    const client = new RelayClient(SECRET, { onSessions: (s) => (got = s) });
    clients.push(client);
    client.connect();
    const ws = await nextWs(base);
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });

    ws.deliver(await sealedMsg({ event: "sessions", sessions }));
    await waitFor(() => got !== null, "sessions 帧解密后分发"); // Unseal is async
    expect(got).toEqual(sessions);
  });

  it("sessions_delta performs keyed upsert/remove on full baseline and resorts by lastActivityMs", async () => {
    let got: Array<{ id: string; lastActivityMs: number; status?: string }> = [];
    const base = FakeWs.instances.length;
    const client = new RelayClient(SECRET, {
      onSessions: (s) => (got = s as typeof got),
    });
    clients.push(client);
    client.connect();
    const ws = await nextWs(base);
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });

    // Full baseline (desktop slim already sorted by lastActivityMs desc).
    const full = [
      { id: "s1", lastActivityMs: 3, status: "active" },
      { id: "s2", lastActivityMs: 2, status: "idle" },
      { id: "s3", lastActivityMs: 1, status: "idle" },
    ];
    ws.deliver(await sealedMsg({ event: "sessions", sessions: full }));
    await waitFor(() => got.length === 3, "整表基线到达");
    expect(got.map((s) => s.id)).toEqual(["s1", "s2", "s3"]);

    // Delta: s2 activity rises to 9, s4 added (4), s3 removed.
    ws.deliver(
      await sealedMsg({
        event: "sessions_delta",
        upsert: [
          { id: "s2", lastActivityMs: 9, status: "active" },
          { id: "s4", lastActivityMs: 4, status: "active" },
        ],
        remove: ["s3"],
      }),
    );
    // After s3 is removed, the table is still 3 rows (s2/s4/s1), so length alone doesn't show if delta applied —
    // watch s3 disappearing, a condition that only holds after delta is applied.
    await waitFor(() => !got.some((s) => s.id === "s3"), "sessions_delta 应用");
    // After merge, sorted by lastActivityMs desc: s2(9) s4(4) s1(3); s3 removed.
    expect(got.map((s) => s.id)).toEqual(["s2", "s4", "s1"]);
    expect(got.find((s) => s.id === "s2")?.status).toBe("active");
  });

  it("Desktop frame gzipped then encrypted (z:true) is decrypted, inflated, then distributed", async () => {
    const sessions = [{ id: "g1", workspaceName: "delta", status: "active" }];
    let got: unknown = null;
    const base = FakeWs.instances.length;
    const client = new RelayClient(SECRET, { onSessions: (s) => (got = s) });
    clients.push(client);
    client.connect();
    const ws = await nextWs(base);
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });

    // Desktop path: payload is first gzipped to bytes, then sealed to those bytes, envelope marked z:true.
    const gz = await gzipBytes(JSON.stringify({ event: "sessions", sessions }));
    const sealed = await sealBytes(KEYS.encKey, gz);
    ws.deliver({ type: "msg", payload: { ...sealed, z: true } });

    await waitFor(() => got !== null, "gzip 帧解密 + inflate 后分发");
    expect(got).toEqual(sessions);
  });

  it("Non-ciphertext (non-{enc:box}) msg payload is discarded without crashing", async () => {
    let got: unknown = "UNTOUCHED";
    const base = FakeWs.instances.length;
    const client = new RelayClient(SECRET, { onSessions: (s) => (got = s) });
    clients.push(client);
    client.connect();
    const ws = await nextWs(base);
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });

    // Under strict encryption, plaintext business payload should never be processed (may come from untrusted source).
    ws.deliver({ type: "msg", payload: { event: "sessions", sessions: [{ id: "x" }] } });
    // Negative assertion: give "plaintext frame incorrectly processed" a chance; fixed sleep is the right tool.
    await tick();
    expect(got).toBe("UNTOUCHED");
  });
});

describe("RelayClient client_hello carries build commit", () => {
  const clients: RelayClient[] = [];

  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  // Desktop uses appCommit in hello to judge if this phone's bundle is stale, so hello frame must
  // pass through deviceInfo.appCommit as-is (sendHello does `...info` spread; lock it down here).
  it("hello frame after authed carries deviceInfo.appCommit", async () => {
    const base = FakeWs.instances.length;
    const client = new RelayClient(SECRET, {}, () => ({
      clientId: "c-1",
      label: "iPhone",
      platform: "ios",
      pushSubscribed: false,
      supportsGzip: true,
      supportsBinary: true,
      supportsDelta: true,
      appCommit: "abc1234",
    }));
    clients.push(client);
    client.connect();
    const ws = await nextWs(base);
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });

    await waitFor(
      async () => (await openSent(ws)).some((p) => p.event === "client_hello"),
      "client_hello 发出",
    );
    const hello = (await openSent(ws)).find((p) => p.event === "client_hello");
    expect(hello).toBeTruthy();
    expect(hello!.appCommit).toBe("abc1234");
  });
});

// answerViaReq: decision card replies use req/reply (not the old fire-and-forget answer frame); over weak
// networks, either get desktop delivery confirmation, retry, or ultimately fail while caller keeps the card.
// This fixes the bug where "answer went through, card vanished, app still waiting" — never treat card as
// answered before getting delivery confirmation.
describe("RelayClient.answerViaReq weak network delivery confirmation", () => {
  const clients: RelayClient[] = [];
  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });
  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  /** Unseal all decision_answer req frame req_ids sent from a connection (in order sent). */
  async function answerReqIds(ws: FakeWs): Promise<string[]> {
    const out: string[] = [];
    for (const p of await openSent(ws)) {
      if (p.event === "req" && p.method === "decision_answer") out.push(String(p.req_id));
    }
    return out;
  }
  async function waitAnswerReqCount(ws: FakeWs, n: number): Promise<void> {
    for (let i = 0; i < 400; i++) {
      if ((await answerReqIds(ws)).length >= n) return;
      await tick(1);
    }
    throw new Error(`未在预期时间内发出 ${n} 个 decision_answer req`);
  }

  it("Sends decision_answer req, resolves after receiving ok:true reply", async () => {
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "elicitation",
      "d-confirm",
      { declined: false, answers: { q1: "a" } },
      { attempts: 3, timeoutMs: 1000 },
    );
    p.catch(() => {});
    await waitAnswerReqCount(ws, 1);
    const frame = (await openSent(ws)).find(
      (f) => f.event === "req" && f.method === "decision_answer",
    );
    expect(frame).toBeTruthy();
    const params = frame!.params as Record<string, unknown>;
    expect(params.kind).toBe("elicitation");
    expect(params.id).toBe("d-confirm");
    expect((params.answers as Record<string, unknown>).q1).toBe("a");

    const reqId = (await answerReqIds(ws))[0];
    ws.deliver(await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: null }));
    await expect(p).resolves.toBeUndefined();
  });

  it("reply lost (no verdict) → resend, resolves only on second ok:true", async () => {
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "fleet-ask",
      "d-resend",
      { cancelled: false, answers: {} },
      // 200ms instead of 50ms: this uses **real** timers, and the assertion comes after the second frame is sent.
      // With 50ms on a loaded machine, the second frame also times out, so the entire promise rejects —
      // actual tests showed this twice; took two rounds to realize it was test-internal race, not the code.
      // We're relaxing the delivery window, not the tested semantics: the first frame must still timeout to trigger resend.
      { attempts: 2, timeoutMs: 200 },
    );
    p.catch(() => {});
    // After first frame times out (not delivered), second frame should auto-send.
    await waitAnswerReqCount(ws, 2);
    const reqs = await answerReqIds(ws);
    expect(reqs.length).toBe(2);
    // Reply ok:true to second frame → overall resolve (resend is idempotent-deduplicated by desktop, safe).
    ws.deliver(await sealedMsg({ event: "reply", req_id: reqs[1], ok: true, data: null }));
    await expect(p).resolves.toBeUndefined();
  });

  it("Desktop verdict ok:false → no resend, reject immediately (remote)", async () => {
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "elicitation",
      "d-verdict",
      { declined: false, answers: {} },
      { attempts: 3, timeoutMs: 1000 },
    );
    await waitAnswerReqCount(ws, 1);
    const reqId = (await answerReqIds(ws))[0];
    ws.deliver(
      await sealedMsg({ event: "reply", req_id: reqId, ok: false, error: "no pending request" }),
    );
    const err = await p.catch((e) => e);
    expect(isDesktopRejection(err)).toBe(true);
    // Explicit rejection should not trigger resend: only one frame sent.
    expect((await answerReqIds(ws)).length).toBe(1);
  });

  it("Resend budget exhausted with no verdict → reject (non-remote)", async () => {
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "elicitation",
      "d-exhaust",
      { declined: false, answers: {} },
      { attempts: 2, timeoutMs: 5 },
    );
    const err = await p.catch((e) => e);
    expect(err).toBeInstanceOf(RelayRequestError);
    expect((err as RelayRequestError).remote).toBe(false);
    expect((await answerReqIds(ws)).length).toBe(2);
  });

  it("Old desktop replies unknown method → fall back to fire-and-forget answer(), resolve", async () => {
    // A desktop that predates decision_answer rejects it as an unknown method.
    // A new phone must still be able to answer it: fall back to the legacy
    // fire-and-forget `answer` frame (no worse than that old desktop's behaviour)
    // rather than stranding the card — and must NOT keep resending the req.
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "elicitation",
      "d-oldserver",
      { declined: false, answers: {} },
      { attempts: 3, timeoutMs: 1000 },
    );
    await waitAnswerReqCount(ws, 1);
    const reqId = (await answerReqIds(ws))[0];
    ws.deliver(
      await sealedMsg({
        event: "reply",
        req_id: reqId,
        ok: false,
        error: "unknown method: decision_answer",
      }),
    );
    // Resolves (best-effort fallback sent), not rejects.
    await expect(p).resolves.toBeUndefined();
    // Exactly one decision_answer req (no resend), plus a legacy `answer` frame.
    expect((await answerReqIds(ws)).length).toBe(1);
    const legacy = (await openSent(ws)).find((f) => f.event === "answer");
    expect(legacy).toBeTruthy();
    expect(legacy!.kind).toBe("elicitation");
    expect(legacy!.id).toBe("d-oldserver");
  });

  // ── Relay store-and-forward delivery ────────────────────────────────────
  //
  // When desktop goes offline, relay takes over reply frames and forwards them when desktop returns,
  // immediately replying with msg_ack{status:"queued"}. Phone connections live ~13 seconds, not long
  // enough to wait for desktop reconnect, so "relay has it safely" must count as delivery complete —
  // otherwise user submits, reply is actually in relay's hands, but card reports failure because it
  // never got desktop's reply.

  /** Get raw outer frames sent from a connection (unencrypted parts, for asserting ack_id). */
  function rawSent(ws: FakeWs): Array<Record<string, unknown>> {
    return ws.sent.map((s) => JSON.parse(s) as Record<string, unknown>);
  }

  it("Answer frame carries outer ack_id; relay response queued counts as delivery complete", async () => {
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "fleet-ask",
      "d-queued",
      { cancelled: false, answers: { q: "v" } },
      { attempts: 3, timeoutMs: 1000 },
    );
    p.catch(() => {});
    await waitAnswerReqCount(ws, 1);

    // Outer frame must carry ack_id: relay can't unseal the payload, doesn't see the req_id inside.
    const msgFrame = rawSent(ws).find((f) => f.type === "msg" && f.ack_id);
    expect(msgFrame).toBeTruthy();
    const ackId = String(msgFrame!.ack_id);

    // Desktop offline, relay says "I have it now". This frame should land the reply.
    ws.deliver({ type: "msg_ack", ack_id: ackId, status: "queued" });
    await expect(p).resolves.toBeUndefined();
    // Already delivered, should not resend.
    expect((await answerReqIds(ws)).length).toBe(1);
  });

  it("relay responds dropped → treat as undelivered, continue resending", async () => {
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "elicitation",
      "d-dropped",
      { declined: false, answers: {} },
      { attempts: 2, timeoutMs: 1000 },
    );
    p.catch(() => {});
    await waitAnswerReqCount(ws, 1);
    const ackId = String(rawSent(ws).find((f) => f.type === "msg" && f.ack_id)!.ack_id);

    ws.deliver({ type: "msg_ack", ack_id: ackId, status: "dropped" });
    // Not delivered → resend second frame instead of waiting 1000ms timeout.
    await waitAnswerReqCount(ws, 2);
    expect((await answerReqIds(ws)).length).toBe(2);
  });

  it("delivered ack does not replace desktop verdict: still wait for reply", async () => {
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "elicitation",
      "d-delivered",
      { declined: false, answers: {} },
      { attempts: 1, timeoutMs: 1000 },
    );
    p.catch(() => {});
    await waitAnswerReqCount(ws, 1);
    const ackId = String(rawSent(ws).find((f) => f.type === "msg" && f.ack_id)!.ack_id);

    // Desktop online, relay has forwarded — but only desktop knows if it actually consumed this card,
    // so delivered cannot replace reply (else we're back to the original "optimistic card move" bug).
    ws.deliver({ type: "msg_ack", ack_id: ackId, status: "delivered" });
    let settled = false;
    p.then(
      () => (settled = true),
      () => (settled = true),
    );
    // Negative assertion: give "delivered/queued treated as final" a chance; fixed sleep is the right tool.
    await tick(5);
    expect(settled).toBe(false);

    const reqId = (await answerReqIds(ws))[0];
    ws.deliver(await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: null }));
    await expect(p).resolves.toBeUndefined();
  });

  it("Regular data requests don't treat queued as a result", async () => {
    // Only decision replies accept "relay has it safely" as final; pending_snapshot and similar
    // requests want data from the desktop, relay's store confirmation means nothing to them.
    const { client, ws } = await connected(clients);
    const p = client.request("pending_snapshot", {}, 1000);
    p.catch(() => {});
    await tick(2);
    const ackId = rawSent(ws).find((f) => f.type === "msg" && f.ack_id)?.ack_id;
    if (ackId) {
      ws.deliver({ type: "msg_ack", ack_id: String(ackId), status: "queued" });
    }
    let settled = false;
    p.then(
      () => (settled = true),
      () => (settled = true),
    );
    // Negative assertion: give "delivered/queued treated as final" a chance; fixed sleep is the right tool.
    await tick(5);
    expect(settled).toBe(false);
  });
});

describe("relayDisplayHost", () => {
  it("Production relay shows only hostname (https is normal, scheme is noise)", () => {
    expect(relayDisplayHost("https://fleet-relay.muveeai.com")).toBe("fleet-relay.muveeai.com");
    expect(relayDisplayHost("https://fleet-relay.muveeai.com/")).toBe("fleet-relay.muveeai.com");
  });

  it("Non-https (local dev relay) keeps scheme and port — that's the difference to spot at a glance", () => {
    expect(relayDisplayHost("http://127.0.0.1:18080")).toBe("http://127.0.0.1:18080");
  });

  it("If unparseable, return as-is without throwing", () => {
    expect(relayDisplayHost("not a url")).toBe("not a url");
  });
});

describe("resolveRelayBase", () => {
  const BAKED = "https://fleet-relay.muveeai.com";
  const SHELL_ORIGIN = "https://fleet.local";

  it("QR code relay from pairing overrides the one baked at build time", () => {
    // Harmony shell scans a QR code pointing to a self-hosted relay. Previously WebShell
    // only passed secret to the page; relay was always the compile-time baked one — a
    // self-hosted relay couldn't connect on Harmony at all.
    const hash = "#k=deadbeefdeadbeef&relay=" + encodeURIComponent("https://relay.corp.example.com");
    expect(resolveRelayBase(hash, BAKED, SHELL_ORIGIN)).toBe("https://relay.corp.example.com");
  });

  it("Without relay, use baked value; PWA falls back to same origin", () => {
    expect(resolveRelayBase("#k=abc", BAKED, SHELL_ORIGIN)).toBe(BAKED);
    expect(resolveRelayBase("#k=abc", undefined, "https://fleet-relay.muveeai.com")).toBe(
      "https://fleet-relay.muveeai.com",
    );
  });

  it("Non-http(s) relay is always ignored, falls back to baked value", () => {
    for (const bad of ["javascript:alert(1)", "fleet-relay.muveeai.com", "ftp://x/y", ""]) {
      expect(resolveRelayBase("#k=abc&relay=" + encodeURIComponent(bad), BAKED, SHELL_ORIGIN)).toBe(
        BAKED,
      );
    }
  });
});
