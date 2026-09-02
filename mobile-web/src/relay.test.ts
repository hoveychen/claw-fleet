import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { RelayClient, RelayRequestError, type RttSample, isDesktopRejection } from "./relay";
// URL 归属计算搬去了 relayBase.ts（一个不含 relay 客户端的叶子模块）。
import { relayDisplayHost, resolveRelayBase } from "./relayBase";
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
    await tick();
    const reqId = await sentReqId(ws);

    // 早 ack 到达：onAck 触发，但 promise 不 resolve（仍等最终 reply）。
    ws.deliver(await sealedMsg({ event: "ack", req_id: reqId }));
    await tick();
    expect(acked).toBe(true);
    expect(settled).toBe("PENDING");

    // 最终 reply 到达才 resolve。
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
    await tick();
    const reqId = await sentReqId(ws);
    ws.deliver(await sealedMsg({ event: "ack", req_id: reqId }));
    ws.deliver(await sealedMsg({ event: "ack", req_id: reqId }));
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

  /** 发一个请求并返回它的 req_id 与收到的样本槽。 */
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
    await tick();
    return { ws, reqId: await sentReqId(ws), samples };
  }

  it("ack 与 handle_ms 都在时，三段可分辨", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    // relay 先回 msg_ack（此时桌面还没参与），隔一会儿桌面的 reply 才到。
    ws.deliver({ type: "msg_ack", ack_id: reqId, status: "delivered" });
    await tick(40);
    ws.deliver(
      await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: {}, handle_ms: 380 }),
    );
    await tick();

    expect(samples).toHaveLength(1);
    const s = samples[0];
    expect(s.desktopHandleMs).toBe(380);
    expect(s.phoneRelayMs).not.toBeNull();
    // ack 在 reply 之前到，所以手机段必然短于总数——若把两者搞反，残差会变负。
    expect(s.phoneRelayMs!).toBeLessThan(s.totalMs);
    expect(s.totalMs).toBeGreaterThanOrEqual(40);
  });

  it("msg_ack 没赶上时手机段报 null，不冒充 0", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    ws.deliver(
      await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: {}, handle_ms: 12 }),
    );
    await tick();
    expect(samples[0].phoneRelayMs).toBeNull();
    expect(samples[0].desktopHandleMs).toBe(12);
  });

  it("旧桌面不带 handle_ms 时桌面段报 null，不冒充 0", async () => {
    const { ws, reqId, samples } = await requestWithSamples();
    ws.deliver({ type: "msg_ack", ack_id: reqId, status: "delivered" });
    await tick();
    ws.deliver(await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: {} }));
    await tick();
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
    await tick();
    expect(samples).toHaveLength(1);
    expect(samples[0].desktopHandleMs).toBe(7);
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

    // 整表基线（桌面 slim 已按 lastActivityMs desc 排好）。
    const full = [
      { id: "s1", lastActivityMs: 3, status: "active" },
      { id: "s2", lastActivityMs: 2, status: "idle" },
      { id: "s3", lastActivityMs: 1, status: "idle" },
    ];
    ws.deliver(await sealedMsg({ event: "sessions", sessions: full }));
    await tick();
    expect(got.map((s) => s.id)).toEqual(["s1", "s2", "s3"]);

    // 增量：s2 活跃度升到 9、s4 新增(4)、s3 删除。
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
    await tick();
    // 合并后按 lastActivityMs desc: s2(9) s4(4) s1(3)；s3 已移除。
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

  // 桌面端靠 hello 里的 appCommit 判断这台手机跑的 bundle 是否过期，所以 hello 帧
  // 必须原样带上 deviceInfo 的 appCommit（sendHello 是 `...info` 展开，这里锁住它）。
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

    await tick();
    const hello = (await openSent(ws)).find((p) => p.event === "client_hello");
    expect(hello).toBeTruthy();
    expect(hello!.appCommit).toBe("abc1234");
  });
});

// answerViaReq:决策卡答复走 req/reply(而非旧的即发即忘 answer 帧),弱网下要么拿到
// 桌面送达确认、要么重发、要么最终失败让调用方保留卡片。这是「弱网答复了、卡片却消失
// 且 app 端仍在等」这个 bug 的修复面——绝不在拿到送达确认前把卡当成已答。
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

  /** 解出某连接发出的所有 decision_answer req 帧的 req_id(按发出顺序)。 */
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
      { attempts: 2, timeoutMs: 50 },
    );
    p.catch(() => {});
    // 第一帧超时(50ms,不投递)后应自动发第二帧。
    await waitAnswerReqCount(ws, 2);
    const reqs = await answerReqIds(ws);
    expect(reqs.length).toBe(2);
    // 给第二帧回 ok:true → 整体 resolve(重发被桌面幂等去重,安全)。
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
    // 明确裁决不该触发重发:只发了一帧。
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

  // ── relay 托管交付(store-and-forward)────────────────────────────────────
  //
  // 桌面掉线时 relay 会接管答复帧并在桌面回来后转投,同时立刻回一个
  // msg_ack{status:"queued"}。手机连接中位只活 13 秒,等不到桌面重连,所以
  // 「relay 已收妥」就必须算交付完成 —— 否则老板按了提交、答复其实已在
  // relay 手里,卡片却因为等不到桌面 reply 而报失败。

  /** 取某连接发出的原始外层帧(未加密部分,用来断言 ack_id)。 */
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

    // 外层必须带 ack_id:relay 打不开密封 payload,读不到里面的 req_id。
    const msgFrame = rawSent(ws).find((f) => f.type === "msg" && f.ack_id);
    expect(msgFrame).toBeTruthy();
    const ackId = String(msgFrame!.ack_id);

    // 桌面没上线,relay 报「已接管」。这一帧就该让答复落地。
    ws.deliver({ type: "msg_ack", ack_id: ackId, status: "queued" });
    await expect(p).resolves.toBeUndefined();
    // 已交付就不该再重发。
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
    // 没送达 → 重发第二帧,而不是坐等 1000ms 超时。
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

    // 桌面在线、relay 已转交 —— 但桌面是否真消费了这张卡只有它自己知道,
    // 所以 delivered 不能顶替 reply(否则又回到「乐观移卡」那个原始 bug)。
    ws.deliver({ type: "msg_ack", ack_id: ackId, status: "delivered" });
    let settled = false;
    p.then(
      () => (settled = true),
      () => (settled = true),
    );
    await tick(5);
    expect(settled).toBe(false);

    const reqId = (await answerReqIds(ws))[0];
    ws.deliver(await sealedMsg({ event: "reply", req_id: reqId, ok: true, data: null }));
    await expect(p).resolves.toBeUndefined();
  });

  it("普通数据请求不把 queued 当成结果", async () => {
    // 只有决策答复愿意接受「relay 已收妥」作为终局;pending_snapshot 这类
    // 请求要的是桌面的数据,relay 的托管确认对它毫无意义。
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
