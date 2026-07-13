import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { RelayClient, RelayRequestError, isDesktopRejection } from "./relay";

// relay.ts 依赖浏览器全局（window.setTimeout/WebSocket/location），node 环境没有，
// 用最小 shim 注入，并用假 WebSocket 捕获发出的帧、手动投递收到的帧。

class FakeWs {
  static OPEN = 1;
  static instances: FakeWs[] = [];
  readyState = FakeWs.OPEN;
  binaryType = "blob";
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string | ArrayBuffer }) => void) | null = null;
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
  /** 模拟 relay 投递一个二进制帧（压缩后的 msg payload）。 */
  deliverBinary(buf: ArrayBuffer) {
    this.onmessage?.({ data: buf });
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
    for (const c of clients.splice(0)) c.close();
  });

  it("桌面端 ok:false 的 reply → remote 错误，携带桌面原文", async () => {
    const { client, ws } = connected();
    clients.push(client);
    const p = client.request("spawn_session", { workspacePath: "~/nope" });
    ws.deliver({
      type: "msg",
      payload: {
        event: "reply",
        req_id: sentReqId(ws),
        ok: false,
        error: "Workspace directory not found: /Users/x/nope",
      },
    });
    const err = await p.catch((e) => e);
    expect(err).toBeInstanceOf(RelayRequestError);
    expect((err as RelayRequestError).remote).toBe(true);
    expect((err as Error).message).toContain("Workspace directory not found");
    expect(isDesktopRejection(err)).toBe(true);
  });

  it("请求超时（帧可能丢了）→ 非 remote 错误，调用方仍可进宽限期", async () => {
    const { client, ws } = connected();
    clients.push(client);
    const p = client.request("spawn_session", {}, 5); // 5ms 超时，不投递 reply
    void ws;
    const err = await p.catch((e) => e);
    expect(err).toBeInstanceOf(RelayRequestError);
    expect((err as RelayRequestError).remote).toBe(false);
    expect(isDesktopRejection(err)).toBe(false);
  });

  it("未连接 → 非 remote 错误", async () => {
    const client = new RelayClient("shared-secret-1234567890", {});
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

/** 用与桌面端对称的方式 gzip 一段 JSON，返回原始字节（二进制帧夹具）。 */
async function gzipBytes(json: string): Promise<ArrayBuffer> {
  const stream = new Blob([new TextEncoder().encode(json)])
    .stream()
    .pipeThrough(new CompressionStream("gzip"));
  return new Response(stream).arrayBuffer();
}

/** 用与桌面端对称的方式（gzip + base64）压一段 JSON，供解压测试当夹具。 */
async function gzipBase64(json: string): Promise<string> {
  const buf = new Uint8Array(await gzipBytes(json));
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

  it("通用 {enc:\"gzip\",data} 文本信封被解压后按事件分发", async () => {
    const sessions = [{ id: "g1", workspaceName: "delta", status: "active" }];
    let got: unknown = null;
    const client = new RelayClient("shared-secret-1234567890", {
      onSessions: (s) => (got = s),
    });
    client.connect();
    const ws = FakeWs.instances[FakeWs.instances.length - 1];
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });

    // 整个 payload 被压进 data —— 解压后是 {event:sessions, sessions:[...]}。
    const data = await gzipBase64(JSON.stringify({ event: "sessions", sessions }));
    ws.deliver({ type: "msg", payload: { enc: "gzip", data } });

    await new Promise((r) => setTimeout(r, 20));
    expect(got).toEqual(sessions);
    client.close();
  });

  it("二进制帧被解压后按事件分发", async () => {
    const sessions = [{ id: "b1", workspaceName: "epsilon", status: "idle" }];
    let got: unknown = null;
    const client = new RelayClient("shared-secret-1234567890", {
      onSessions: (s) => (got = s),
    });
    client.connect();
    const ws = FakeWs.instances[FakeWs.instances.length - 1];
    ws.onopen?.();
    ws.deliver({ type: "authed", agent_online: true, clients: 1 });
    expect(ws.binaryType).toBe("arraybuffer");

    // 二进制帧的内容就是压缩后的 payload 本身（relay 原样透传）。
    const buf = await gzipBytes(JSON.stringify({ event: "sessions", sessions }));
    ws.deliverBinary(buf);

    await new Promise((r) => setTimeout(r, 20));
    expect(got).toEqual(sessions);
    client.close();
  });
});
