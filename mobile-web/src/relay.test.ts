import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { RelayClient, RelayRequestError, isDesktopRejection } from "./relay";
import { deriveKeys, isSealed, open, type RelayKeys, seal, sealBytes } from "./relayCrypto";

// relay.ts 依赖浏览器全局（window.setTimeout/WebSocket/location），node 环境没有，
// 用最小 shim 注入，并用假 WebSocket 捕获发出的帧、手动投递收到的帧。
//
// 端到端加密后每个 `msg.payload` 都是密文信封 {enc:"box"}（auth/authed/error 等
// relay 控制帧仍是明文）。所以：捕获的出站帧要先用同一对配对密钥解密才能断言；
// 注入的入站业务帧要先加密再投递。密钥两端一致（同 secret 派生同 key），这正是
// relay 广播串号场景成立的前提。测试跑在 node，全局有 crypto.subtle。

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
  /** 模拟 relay 投递一个文本帧给这个连接。 */
  deliver(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}

const tick = (ms = 20) => new Promise((r) => setTimeout(r, ms));

/** 连接是异步建立的（open() 先 await 派生密钥再 new WebSocket），所以等到新的
 *  FakeWs 出现为止再返回它。 */
async function nextWs(baseline: number): Promise<FakeWs> {
  for (let i = 0; i < 200; i++) {
    if (FakeWs.instances.length > baseline) return FakeWs.instances[FakeWs.instances.length - 1];
    await tick(1);
  }
  throw new Error("ws 未在预期时间内创建");
}

/** 把一个业务 payload 封成桌面会发出的密文 `msg` 帧（z=false，明文直封）。 */
async function sealedMsg(payload: unknown): Promise<{ type: string; payload: unknown }> {
  return { type: "msg", payload: await seal(KEYS.encKey, JSON.stringify(payload)) };
}

/** 解出某连接发出的所有密文 `msg` 业务 payload。 */
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

/** 取某连接发出的 `req` 帧的 req_id（先解密）。 */
async function sentReqId(ws: FakeWs): Promise<string> {
  for (const p of await openSent(ws)) {
    if (p.event === "req") return String(p.req_id);
  }
  throw new Error("该连接没有发出 req 帧");
}

const windowShim = () => ({
  location: { origin: "http://localhost" },
  setTimeout: (fn: () => void, ms?: number) => setTimeout(fn, ms) as unknown as number,
  clearTimeout: (id: number) => clearTimeout(id),
  setInterval: (fn: () => void, ms?: number) => setInterval(fn, ms) as unknown as number,
  clearInterval: (id: number) => clearInterval(id),
});

/** 建一个已 authed 的 client，返回它和它的假 ws。 */
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
    // channelToken 是 64 hex（HKDF 256 bit），relay 眼里只是个不透明 token。
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
    // 清掉 15s 超时/心跳定时器，避免泄漏到别的测试。
    for (const c of clients.splice(0)) c.close();
  });

  // 核心 bug：两台手机同 secret 落到同一 relay channel。relay 把 agent 的 reply 帧
  // 广播给该 channel 的每一个 client（registry.rs forward），帧里没有客户端定向。
  // 若两台手机的 req_id 空间共享（都从 r1 起），A 的 reply 会被 B 拿自己的同号
  // pending 匹配掉，B 解析出 A 的数据。同 secret 派生同 encKey，所以广播来的密文
  // B 也能解开——定向只靠 reqPrefix（每实例一个 UUID）区分。
  it("A 的 reply 被广播到 B 时，B 不会用它 resolve 自己的同号请求", async () => {
    const a = await connected(clients);
    const b = await connected(clients);

    const pa = a.client.request<{ who: string }>("tail", { session: "A" });
    const pb = b.client.request<{ who: string }>("pending_snapshot", {});

    let bSettled: unknown = "PENDING";
    pb.then((v) => (bSettled = v)).catch(() => (bSettled = "REJECTED"));

    // 出站是异步加密的，等 req 帧真正落到 sent 上。
    await tick();
    const reqIdA = await sentReqId(a.ws);

    // agent 对 A 请求的回复（携带 A 的 req_id），被 relay 广播到 A 和 B 两个连接。
    const replyForA = await sealedMsg({
      event: "reply",
      req_id: reqIdA,
      ok: true,
      data: { who: "A-tail" },
    });
    a.ws.deliver(replyForA);
    b.ws.deliver(replyForA); // 广播泄漏到 B

    await expect(pa).resolves.toEqual({ who: "A-tail" });

    // 让 microtask/timer 有机会跑，再断言 B 没被 A 的回复串号。
    await tick();
    expect(bSettled).toBe("PENDING");
  });
});

// 一个请求可能因为两种完全不同的原因失败，调用方的正确反应也完全相反：
//   - 桌面端明确回了 ok:false（"Workspace directory not found: ..."）——桌面收到了、
//     判断了、拒绝了。重试/等待都没有意义，该立刻把错误摊给用户。
//   - reply 帧丢了（切网/息屏/重连，relay 是 best-effort 不排队）——桌面很可能已经
//     照做了，只是回执没回来。这时才该进宽限期盯快照。
// 以前两者都是裸 Error，Composer 无法区分，于是把桌面的明确拒绝也拖进 20 秒宽限期，
// 表现成"点了没反应，二十秒后才报错"。
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
    await tick();
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

/** 用与桌面端对称的方式 gzip 一段 JSON，返回原始字节（模拟桌面 z:true 的明文）。 */
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
    await tick(); // 解密是异步的
    expect(got).toEqual(sessions);
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

    // 桌面路径：payload 整体先 gzip 成字节，再对字节 seal，信封打 z:true。
    const gz = await gzipBytes(JSON.stringify({ event: "sessions", sessions }));
    const sealed = await sealBytes(KEYS.encKey, gz);
    ws.deliver({ type: "msg", payload: { ...sealed, z: true } });

    await tick();
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

    // 永远加密下，明文业务 payload 不该被处理（可能来自不受信来源）。
    ws.deliver({ type: "msg", payload: { event: "sessions", sessions: [{ id: "x" }] } });
    await tick();
    expect(got).toBe("UNTOUCHED");
  });
});
