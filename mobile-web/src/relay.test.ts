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

describe("RelayClient 连接用 channelToken 认证", () => {
  const clients: RelayClient[] = [];
  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });
  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  it("auth 帧发的是派生 channelToken，不是原始 secret", async () => {
    const { ws } = await connected(clients);
    const authFrame = ws.sent.map((s) => JSON.parse(s)).find((f) => f.type === "auth");
    expect(authFrame).toBeTruthy();
    expect(authFrame.secret).toBe(KEYS.channelToken);
    expect(authFrame.secret).not.toBe(SECRET);
    // channelToken is 64 hex (HKDF 256 bit); relay sees it as an opaque token.
    expect(authFrame.secret).toMatch(/^[0-9a-f]{64}$/);
  });
});

describe("RelayClient 跨设备 req_id 隔离", () => {
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
  // If both phones share req_id space (both start from 1), A's reply gets matched by B's pending with the
  // same number, and B parses A's data. Same secret derives same encKey, so B can also unseal the broadcast
  // ciphertext — routing only differentiates by reqPrefix (a UUID per instance).
  it("A 的 reply 被广播到 B 时，B 不会用它 resolve 自己的同号请求", async () => {
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
    b.ws.deliver(replyForA); // 广播泄漏到 B

    await expect(pa).resolves.toEqual({ who: "A-tail" });

    // Negative assertion: give microtasks/timers a chance to run, then assert B wasn't cross-wired by A's reply.
    // Must use fixed sleep here — polling "bSettled is still PENDING" returns immediately without verifying.
    await tick();
    expect(bSettled).toBe("PENDING");
  });
});

describe("RelayClient 早 ack(方案 A)", () => {
  const clients: RelayClient[] = [];

  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  it("收到 ack 触发 onAck，但 promise 仍待 reply 才 resolve", async () => {
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

  it("重复 ack 只触发一次 onAck", async () => {
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

// 整条往返（手机→relay→桌面→relay→手机）是五段之和，只报总数时"卡"没法归因。
// 两个已有的观测点各切一刀：relay 收到上行帧就立刻回的 msg_ack 圈出手机↔relay 那
// 一段（不含桌面），桌面盖印在 reply 里的 handle_ms 圈出它自己 handler 的耗时。
// 剩下的残差才是 relay↔桌面。任一刀缺席时必须报 null 而不是 0——0 会谎称"那段是
// 零耗时"，把别人的时间算到残差头上。
describe("RelayClient RTT 分段", () => {
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

  it("msg_ack 没赶上时手机段报 null，不冒充 0", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    ws.deliver(
      await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: {}, handle_ms: 12 }),
    );
    await waitFor(() => samples.length >= 1, "RTT 样本产出");
    expect(samples[0].phoneRelayMs).toBeNull();
    expect(samples[0].desktopHandleMs).toBe(12);
  });

  it("旧桌面不带 handle_ms 时桌面段报 null，不冒充 0", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    ws.deliver({ type: "msg_ack", ack_id: reqId, status: "delivered" });
    await tick();
    ws.deliver(await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: {} }));
    await waitFor(() => samples.length >= 1, "RTT 样本产出");
    expect(samples[0].desktopHandleMs).toBeNull();
    expect(samples[0].phoneRelayMs).not.toBeNull();
  });

  it("桌面拒绝（ok:false）也照样出样本——慢和失败是两回事", async () => {
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
describe("RelayClient 失败来源可区分", () => {
  const clients: RelayClient[] = [];

  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  it("桌面端 ok:false 的 reply → remote 错误，携带桌面原文", async () => {
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

  it("请求超时（帧可能丢了）→ 非 remote 错误，调用方仍可进宽限期", async () => {
    const { client } = await connected(clients);
    const p = client.request("spawn_session", {}, 5); // 5ms 超时，不投递 reply
    const err = await p.catch((e) => e);
    expect(err).toBeInstanceOf(RelayRequestError);
    expect((err as RelayRequestError).remote).toBe(false);
    expect(isDesktopRejection(err)).toBe(false);
  });

  it("未连接 → 非 remote 错误", async () => {
    const client = new RelayClient(SECRET, {});
    clients.push(client);
    const err = await client.request("spawn_session", {}).catch((e) => e);
    expect(isDesktopRejection(err)).toBe(false);
  });

  it("普通 Error / 非 Error 值不会被误判成桌面拒绝", () => {
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

describe("RelayClient sessions 快照收发（加密）", () => {
  const clients: RelayClient[] = [];
  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = windowShim();
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });
  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
  });

  it("密文 sessions 帧被解密后透传（z 缺省，未压缩）", async () => {
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

  it("sessions_delta 在整表基线上 keyed upsert/remove 并按 lastActivityMs 重排", async () => {
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

  it("桌面先 gzip 再加密（z:true）的帧被解密后 inflate 再分发", async () => {
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

  it("非密文（非 {enc:box}）msg payload 被丢弃，不会 crash", async () => {
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

describe("RelayClient client_hello 携带构建 commit", () => {
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
  it("authed 后的 hello 帧带上 deviceInfo.appCommit", async () => {
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
describe("RelayClient.answerViaReq 弱网送达确认", () => {
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

  it("发出 decision_answer req,收到 ok:true reply 后 resolve", async () => {
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

  it("reply 丢失(无裁决)→ 重发,第二次 ok:true 才 resolve", async () => {
    const { client, ws } = await connected(clients);
    const p = client.answerViaReq(
      "fleet-ask",
      "d-resend",
      { cancelled: false, answers: {} },
      // 200ms 而不是 50ms:这条用的是**真**定时器,而断言要在第二帧发出之后再
      // 投递回复。50ms 的预算在机器有负载时会让第二帧也超时,于是整条 promise
      // 变成 reject —— 实测偶发过两次,查了两轮才认出是测试自己的竞态而不是
      // 被测代码。放宽的是投递窗口,不是被测语义:第一帧仍然必须超时才会重发。
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

  it("桌面裁决 ok:false → 不重发,立即 reject(remote)", async () => {
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

  it("耗尽重发预算仍无裁决 → reject(非 remote)", async () => {
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

  it("旧桌面回 unknown method → 回退到即发即忘 answer(),resolve", async () => {
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

  it("答复帧带外层 ack_id,relay 回 queued 即视为交付完成", async () => {
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

  it("relay 回 dropped → 当作未送达,继续重发", async () => {
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

  it("delivered 的 ack 不代替桌面裁决:仍等 reply", async () => {
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

  it("普通数据请求不把 queued 当成结果", async () => {
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
  it("生产 relay 只显示主机名（https 是常态，scheme 是噪音）", () => {
    expect(relayDisplayHost("https://fleet-relay.muveeai.com")).toBe("fleet-relay.muveeai.com");
    expect(relayDisplayHost("https://fleet-relay.muveeai.com/")).toBe("fleet-relay.muveeai.com");
  });

  it("非 https（本地 dev relay）保留 scheme 和端口 —— 这正是要一眼看出的差别", () => {
    expect(relayDisplayHost("http://127.0.0.1:18080")).toBe("http://127.0.0.1:18080");
  });

  it("解析不了就原样回显，不抛", () => {
    expect(relayDisplayHost("not a url")).toBe("not a url");
  });
});

describe("resolveRelayBase", () => {
  const BAKED = "https://fleet-relay.muveeai.com";
  const SHELL_ORIGIN = "https://fleet.local";

  it("配对二维码带来的 relay 胜过打包时烧进去的那个", () => {
    // 鸿蒙壳扫到的二维码指向自建 relay。以前 WebShell 只把 secret 传给页面，
    // relay 永远是编译期烧死的那个 —— 自建 relay 在鸿蒙端根本连不上。
    const hash = "#k=deadbeefdeadbeef&relay=" + encodeURIComponent("https://relay.corp.example.com");
    expect(resolveRelayBase(hash, BAKED, SHELL_ORIGIN)).toBe("https://relay.corp.example.com");
  });

  it("没带 relay 时仍用打包值，PWA 则回落同源", () => {
    expect(resolveRelayBase("#k=abc", BAKED, SHELL_ORIGIN)).toBe(BAKED);
    expect(resolveRelayBase("#k=abc", undefined, "https://fleet-relay.muveeai.com")).toBe(
      "https://fleet-relay.muveeai.com",
    );
  });

  it("非 http(s) 的 relay 一律忽略，回落到打包值", () => {
    for (const bad of ["javascript:alert(1)", "fleet-relay.muveeai.com", "ftp://x/y", ""]) {
      expect(resolveRelayBase("#k=abc&relay=" + encodeURIComponent(bad), BAKED, SHELL_ORIGIN)).toBe(
        BAKED,
      );
    }
  });
});
