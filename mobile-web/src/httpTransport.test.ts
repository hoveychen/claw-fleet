import { describe, expect, it, vi } from "vitest";
import { HttpTransport } from "./httpTransport";
import { isDesktopRejection } from "./transport";
import type { TransportHandlers } from "./transport";

/** 最小 EventSource 替身：只做「注册监听 / 由测试手动投递」这两件事。 */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  readonly listeners = new Map<string, ((e: { data: string }) => void)[]>();
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;

  constructor(readonly url: string) {
    FakeEventSource.last = this;
  }

  addEventListener(type: string, cb: (e: { data: string }) => void) {
    const list = this.listeners.get(type) ?? [];
    list.push(cb);
    this.listeners.set(type, list);
  }

  close() {
    this.closed = true;
  }

  /** 测试侧：模拟服务端推来一条命名事件。 */
  emit(type: string, data: string) {
    for (const cb of this.listeners.get(type) ?? []) cb({ data });
  }
}

function jsonResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as unknown as Response;
}

function make(handlers: TransportHandlers = {}, fetchImpl?: typeof fetch) {
  const transport = new HttpTransport(handlers, {
    fetchImpl: fetchImpl ?? (vi.fn(async () => jsonResponse({ ok: true, data: null })) as unknown as typeof fetch),
    eventSourceImpl: FakeEventSource as unknown as typeof EventSource,
  });
  return transport;
}

describe("HttpTransport.request", () => {
  it("打到 POST /mobile_rpc，body 是 {method, params}，取回 data", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ ok: true, data: { hits: 3 } }));
    const t = make({}, fetchImpl as unknown as typeof fetch);

    const out = await t.request<{ hits: number }>("session_search", { q: "foo" });

    expect(out).toEqual({ hits: 3 });
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/mobile_rpc");
    expect(init.method).toBe("POST");
    expect(JSON.parse(String(init.body))).toEqual({
      method: "session_search",
      params: { q: "foo" },
    });
  });

  // 主机给了裁决 —— 重试改变不了结果，调用方必须能认出来别再兜底等下去。
  it("ok:false 是主机的裁决，抛出的错要认得出 remote", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse({ ok: false, error: "unknown method: nope" }),
    );
    const t = make({}, fetchImpl as unknown as typeof fetch);

    const err = await t.request("nope").catch((e) => e);

    expect(isDesktopRejection(err)).toBe(true);
    expect(String(err.message)).toContain("unknown method");
  });

  // 请求根本没落地（网关 502、断网）—— 主机可能已经把活干了，调用方有权另行确认。
  it("HTTP 层失败不是裁决，remote 必须为 false", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ error: "bad gateway" }, 502));
    const t = make({}, fetchImpl as unknown as typeof fetch);

    const err = await t.request("wiki_list").catch((e) => e);

    // 先确认它真的抛了 —— 只断言 `isDesktopRejection` 为假的话，一个什么都不
    // 做、直接 resolve 的实现也能骗过这条。
    expect(err).toBeInstanceOf(Error);
    expect(isDesktopRejection(err)).toBe(false);
  });
});

describe("HttpTransport 的 SSE 映射", () => {
  it("六类决策卡的 *-request 事件都落到 onDecisionCreated", () => {
    const created: [string, unknown][] = [];
    const t = make({ onDecisionCreated: (kind, req) => created.push([kind, req]) });
    t.connect();

    const es = FakeEventSource.last!;
    es.emit("guard-request", JSON.stringify({ id: "g1" }));
    es.emit("elicitation-request", JSON.stringify({ id: "e1" }));
    es.emit("fleet-ask-request", JSON.stringify({ id: "f1" }));
    es.emit("plan-approval-request", JSON.stringify({ id: "p1" }));
    es.emit("permission-prompt-request", JSON.stringify({ id: "pp1" }));
    es.emit("a2ui-render-request", JSON.stringify({ id: "a1" }));

    expect(created.map(([k]) => k)).toEqual([
      "guard",
      "elicitation",
      "fleet-ask",
      "plan-approval",
      "permission-prompt",
      "a2ui-render",
    ]);
    expect(created[0][1]).toEqual({ id: "g1" });
  });

  // dismissed 帧的 data 是裸的 JSON 字符串（服务端 serde_json::to_string(id)），
  // 不是对象 —— 按对象解会静默丢掉每一次「卡片已被解决」。
  it("*-dismissed 事件带的是裸 id 字符串，落到 onDecisionResolved", () => {
    const resolved: [string, string][] = [];
    const t = make({ onDecisionResolved: (kind, id) => resolved.push([kind, id]) });
    t.connect();

    FakeEventSource.last!.emit("guard-dismissed", JSON.stringify("g1"));

    expect(resolved).toEqual([["guard", "g1"]]);
  });

  it("sessions-updated 带全量列表，同时报 full", () => {
    const seen: unknown[][] = [];
    const kinds: string[] = [];
    const t = make({
      onSessions: (s) => seen.push(s),
      onSessionsKind: (k) => kinds.push(k),
    });
    t.connect();

    FakeEventSource.last!.emit("sessions-updated", JSON.stringify([{ id: "s1" }, { id: "s2" }]));

    expect(seen).toEqual([[{ id: "s1" }, { id: "s2" }]]);
    // 同源下服务端只推全量，没有 delta 通道 —— 谎报 delta 会让 UI 显示一个
    // 并不存在的增量链路。
    expect(kinds).toEqual(["full"]);
  });

  it("连上之后 onStatus 与 onAgentOnline 都为真，isAuthed 跟着为真", () => {
    const status: boolean[] = [];
    const agent: boolean[] = [];
    const t = make({ onStatus: (v) => status.push(v), onAgentOnline: (v) => agent.push(v) });

    expect(t.isAuthed).toBe(false);
    t.connect();
    FakeEventSource.last!.onopen?.();

    expect(status).toEqual([true]);
    // 同源部署里「主机在线」和「这张页面加载出来了」是同一件事：发出这张页面
    // 的进程就是回答 /mobile_rpc 的那一个。
    expect(agent).toEqual([true]);
    expect(t.isAuthed).toBe(true);
  });

  it("close() 之后关掉流并报离线", () => {
    const status: boolean[] = [];
    const t = make({ onStatus: (v) => status.push(v) });
    t.connect();
    FakeEventSource.last!.onopen?.();
    t.close();

    expect(FakeEventSource.last!.closed).toBe(true);
    expect(status).toEqual([true, false]);
    expect(t.isAuthed).toBe(false);
  });
});

describe("HttpTransport 没有的能力", () => {
  // 这条传输层没有推送通道。返回 false 是让「更多」页据此隐掉推送开关；
  // 假装成功会让用户开了开关却永远收不到通知。
  it("push 订阅诚实地返回 false", () => {
    const t = make();
    expect(t.pushSubscribe({ endpoint: "x" })).toBe(false);
    expect(t.pushUnsubscribe({ endpoint: "x" })).toBe(false);
  });
});
