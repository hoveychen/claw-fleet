import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { RelayClient } from "./relay";

// relay.ts 依赖浏览器全局（window.setTimeout/WebSocket/location），node 环境没有，
// 用最小 shim 注入，并用假 WebSocket 捕获发出的帧、手动投递收到的帧。

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
  /** 模拟 relay 投递一帧给这个连接。 */
  deliver(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}

/** 取某连接发出的 `req` 帧的 req_id。 */
function sentReqId(ws: FakeWs): string {
  for (const raw of ws.sent) {
    const f = JSON.parse(raw);
    if (f.type === "msg" && f.payload?.event === "req") return String(f.payload.req_id);
  }
  throw new Error("该连接没有发出 req 帧");
}

/** 建一个已 authed 的 client，返回它和它的假 ws。 */
function connected(): { client: RelayClient; ws: FakeWs } {
  const client = new RelayClient("shared-secret-1234567890", {});
  client.connect();
  const ws = FakeWs.instances[FakeWs.instances.length - 1];
  ws.onopen?.();
  ws.deliver({ type: "authed", agent_online: true, clients: 1 });
  return { client, ws };
}

describe("RelayClient 跨设备 req_id 隔离", () => {
  const clients: RelayClient[] = [];

  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = {
      location: { origin: "http://localhost" },
      setTimeout: (fn: () => void, ms?: number) => setTimeout(fn, ms) as unknown as number,
      clearTimeout: (id: number) => clearTimeout(id),
      setInterval: (fn: () => void, ms?: number) => setInterval(fn, ms) as unknown as number,
      clearInterval: (id: number) => clearInterval(id),
    };
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  afterEach(() => {
    // 清掉 15s 超时/心跳定时器，避免泄漏到别的测试。
    for (const c of clients.splice(0)) c.close();
  });

  // 核心 bug：两台手机同 secret 落到同一 relay channel。relay 把 agent 的 reply 帧
  // 广播给该 channel 的每一个 client（registry.rs forward），帧里没有客户端定向。
  // 若两台手机的 req_id 空间共享（都从 r1 起），A 的 reply 会被 B 拿自己的同号
  // pending 匹配掉，B 解析出 A 的数据。
  it("A 的 reply 被广播到 B 时，B 不会用它 resolve 自己的同号请求", async () => {
    const a = connected();
    const b = connected();
    clients.push(a.client, b.client);

    const pa = a.client.request<{ who: string }>("tail", { session: "A" });
    const pb = b.client.request<{ who: string }>("pending_snapshot", {});

    let bSettled: unknown = "PENDING";
    pb.then((v) => (bSettled = v)).catch(() => (bSettled = "REJECTED"));

    // agent 对 A 请求的回复（携带 A 的 req_id），被 relay 广播到 A 和 B 两个连接。
    const replyForA = {
      type: "msg",
      payload: { event: "reply", req_id: sentReqId(a.ws), ok: true, data: { who: "A-tail" } },
    };
    a.ws.deliver(replyForA);
    b.ws.deliver(replyForA); // 广播泄漏到 B

    await expect(pa).resolves.toEqual({ who: "A-tail" });

    // 让 microtask/timer 有机会跑，再断言 B 没被 A 的回复串号。
    await new Promise((r) => setTimeout(r, 20));
    expect(bSettled).toBe("PENDING");
  });
});

/** 用与桌面端对称的方式（gzip + base64）压一段 JSON，供解压测试当夹具。 */
async function gzipBase64(json: string): Promise<string> {
  const stream = new Blob([new TextEncoder().encode(json)])
    .stream()
    .pipeThrough(new CompressionStream("gzip"));
  const buf = new Uint8Array(await new Response(stream).arrayBuffer());
  let bin = "";
  for (const b of buf) bin += String.fromCharCode(b);
  return btoa(bin);
}

describe("RelayClient sessions 快照解压", () => {
  beforeEach(() => {
    FakeWs.instances = [];
    (globalThis as unknown as { window: unknown }).window = {
      location: { origin: "http://localhost" },
      setTimeout: (fn: () => void, ms?: number) => setTimeout(fn, ms) as unknown as number,
      clearTimeout: (id: number) => clearTimeout(id),
      setInterval: (fn: () => void, ms?: number) => setInterval(fn, ms) as unknown as number,
      clearInterval: (id: number) => clearInterval(id),
    };
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeWs;
  });

  it("enc:\"gzip\" 帧被解压回原始会话数组", async () => {
    const sessions = [
      { id: "s1", workspaceName: "alpha", status: "active" },
      { id: "s2", workspaceName: "beta", status: "idle" },
    ];
    let got: unknown = null;
    const client = new RelayClient("shared-secret-1234567890", {
      onSessions: (s) => (got = s),
    });
    client.connect();
    const ws = FakeWs.instances[FakeWs.instances.length - 1];
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });

    const b64 = await gzipBase64(JSON.stringify(sessions));
    ws.deliver({ type: "msg", payload: { event: "sessions", enc: "gzip", sessions: b64 } });

    // 解压是异步的，给 microtask 一拍。
    await new Promise((r) => setTimeout(r, 20));
    expect(got).toEqual(sessions);
    client.close();
  });

  it("明文数组帧仍按老路径直接透传", async () => {
    const sessions = [{ id: "s9", workspaceName: "gamma", status: "active" }];
    let got: unknown = null;
    const client = new RelayClient("shared-secret-1234567890", {
      onSessions: (s) => (got = s),
    });
    client.connect();
    const ws = FakeWs.instances[FakeWs.instances.length - 1];
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });

    ws.deliver({ type: "msg", payload: { event: "sessions", sessions } });
    expect(got).toEqual(sessions); // 同步路径，无需等待
    client.close();
  });
});
