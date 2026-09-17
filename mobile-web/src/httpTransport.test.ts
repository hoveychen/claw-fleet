import { describe, expect, it, vi } from "vitest";
import { HttpTransport } from "./httpTransport";
import { isDesktopRejection } from "./transport";
import type { TransportHandlers } from "./transport";

/** Minimal EventSource stub: only register listeners and manually deliver events in tests. */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  readonly listeners = new Map<string, ((e: { data: string }) => void)[]>();
  /** Same as browser: 0 CONNECTING / 1 OPEN / 2 CLOSED. */
  readyState = 0;
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

  /** Test side: simulate a named event pushed from server. */
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

  // The host has made a decision — retrying won't change it, callers must recognize it and not wait indefinitely.
  it("ok:false 是主机的裁决，抛出的错要认得出 remote", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse({ ok: false, error: "unknown method: nope" }),
    );
    const t = make({}, fetchImpl as unknown as typeof fetch);

    const err: unknown = await t.request("nope").catch((e) => e);

    expect(isDesktopRejection(err)).toBe(true);
    expect(String((err as Error).message)).toContain("unknown method");
  });

  // Request never landed (gateway 502, network down) — the host may have completed the work, callers can verify independently.
  it("HTTP 层失败不是裁决，remote 必须为 false", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ error: "bad gateway" }, 502));
    const t = make({}, fetchImpl as unknown as typeof fetch);

    const err = await t.request("wiki_list").catch((e) => e);

    // First verify it actually threw — if we only assert `isDesktopRejection` is false,
    // a no-op implementation that resolves directly could pass the test.
    expect(err).toBeInstanceOf(Error);
    expect(isDesktopRejection(err)).toBe(false);
  });
});

describe("HttpTransport 的首屏 catch-up", () => {
  // User-reported bug: on same-origin mobile "Tasks" tab stuck at "Loading tasks…
  // Desktop online, receiving initial snapshot".
  //
  // Root cause: SSE semantics. `sessions-updated` broadcasts only when **sessions change**
  // (hooks_server/mod.rs `if sessions_changed`). The relay path has an extra condition:
  // `|| mobile_clients > prev_mobile_clients`, with the comment "push to new clients too,
  // they need an initial snapshot even if nothing changed". SSE has no equivalent.
  //
  // Result: a late-joining client, if sessions don't change after it connects, never
  // receives any frames; `sessionsLoaded` stays false. Verified: same webui process with
  // a second client stuck on that text after 8s, matching the user's screenshot exactly.
  //
  // Fix on client not server: SSE broadcasts to all connections, re-pushing full state
  // for one new client disturbs everyone; but pulling catch-up on mount is what HTTP
  // clients should do anyway — desktop webui's liveProxy does this (calls list_sessions
  // on mount).
  it("connect() 之后主动拉一次 /sessions,不能只等 SSE 推", async () => {
    const seen: unknown[][] = [];
    const kinds: string[] = [];
    const calls: string[] = [];
    const fetchImpl = vi.fn(async (url: string) => {
      calls.push(url);
      if (String(url).endsWith("/sessions")) {
        return jsonResponse([{ id: "s1" }, { id: "s2" }]);
      }
      return jsonResponse({ ok: true, data: null });
    });
    const t = make(
      { onSessions: (s) => seen.push(s), onSessionsKind: (k) => kinds.push(k) },
      fetchImpl as unknown as typeof fetch,
    );

    t.connect();
    // catch-up is async; let the microtask queue finish.
    await vi.waitFor(() => expect(seen.length).toBe(1));

    expect(calls).toContain("/sessions");
    expect(seen[0]).toEqual([{ id: "s1" }, { id: "s2" }]);
    expect(kinds).toEqual(["full"]);
  });

  // catch-up failure shouldn't fail the entire connection — SSE may still deliver data.
  it("catch-up 拉取失败时安静降级,不抛出去", async () => {
    const fetchImpl = vi.fn(async () => {
      throw new Error("network down");
    });
    const t = make({}, fetchImpl as unknown as typeof fetch);

    expect(() => t.connect()).not.toThrow();
    await vi.waitFor(() => expect(fetchImpl).toHaveBeenCalled());
  });
});

describe("connect 时的首屏补拉", () => {
  // User-reported bug: on mobile, tasks page stuck at "Loading tasks…", but wiki works.
  //
  // Root cause: not rendering, but push semantics. Server 2-second loop broadcasts
  // `sessions-updated` only when **session list changes** (hooks_server/mod.rs
  // `if sessions_changed`). Stable container list doesn't change, so new clients never
  // receive the first frame; `sessionsLoaded` stays false. Relay has "force-push full
  // state on new client", SSE doesn't.
  //
  // Verified: curl /events consumes the first frame, second curl gets 0
  // sessions-updated in 8 seconds.
  //
  // So first screen can't wait for push — desktop webui also pulls catch-up on mount,
  // do the same here. Push only handles "changes after".
  it("connect 后主动拉一次 /sessions，不等 SSE", async () => {
    const seen: unknown[][] = [];
    const kinds: string[] = [];
    const fetchImpl = vi.fn(async () => jsonResponse([{ id: "s1" }, { id: "s2" }]));
    const t = new HttpTransport(
      { onSessions: (s) => seen.push(s), onSessionsKind: (k) => kinds.push(k) },
      {
        fetchImpl: fetchImpl as unknown as typeof fetch,
        eventSourceImpl: FakeEventSource as unknown as typeof EventSource,
      },
    );

    t.connect();
    await vi.waitFor(() => expect(seen.length).toBe(1));

    // `vi.fn(async () => …)` parameter type inferred as empty tuple, direct subscript [0][0]
    // is out of bounds (TS2493). Cast to actual call shape first.
    const calls = fetchImpl.mock.calls as unknown as [string][];
    expect(calls[0][0]).toBe("/sessions");
    expect(seen[0]).toEqual([{ id: "s1" }, { id: "s2" }]);
    expect(kinds).toEqual(["full"]);
  });

  // Catch-up failure (gateway 502, network down) must not kill the connection: SSE still connected, changes arrive after.
  it("首屏补拉失败时不抛，也不谎报空列表", async () => {
    const seen: unknown[][] = [];
    const fetchImpl = vi.fn(async () => {
      throw new Error("network down");
    });
    const t = new HttpTransport(
      { onSessions: (s) => seen.push(s) },
      {
        fetchImpl: fetchImpl as unknown as typeof fetch,
        eventSourceImpl: FakeEventSource as unknown as typeof EventSource,
      },
    );

    expect(() => t.connect()).not.toThrow();
    await new Promise((r) => setTimeout(r, 20));
    // Empty array makes UI say "no sessions" — that's asserting a fact we don't know.
    expect(seen).toEqual([]);
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

  // dismissed frame's data is raw JSON string (server-side serde_json::to_string(id)),
  // not an object — parsing as object silently drops each "card resolved".
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
    // Same-origin: server only pushes full, no delta channel — reporting delta would show
    // a non-existent incremental path in UI.
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
    // Same-origin deployment: "host online" and "this page loaded" are the same thing:
    // the process serving this page answers /mobile_rpc.
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

// User-reported bug: webui on server, weak network or reconnect breaks decision cards,
// only page refresh restores them.
//
// Root cause: transport delegates reconnect to EventSource's built-in retry. That
// contract only covers **network layer** disconnect: server returns non-200 (gateway
// 502/504 on weak net) or wrong Content-Type, spec requires UA "fail the connection" —
// readyState becomes CLOSED and never retries. But `connect()` starts with
// `if (this.stream) return`, that dead stream still hangs, no path to rebuild it, only
// page refresh works.
describe("SSE 断流后的自愈", () => {
  function makeWatched(handlers: TransportHandlers = {}) {
    const created: FakeEventSource[] = [];
    class Spy extends FakeEventSource {
      constructor(url: string) {
        super(url);
        created.push(this);
      }
    }
    const transport = new HttpTransport(handlers, {
      fetchImpl: vi.fn(async () =>
        jsonResponse({ ok: true, data: null }),
      ) as unknown as typeof fetch,
      eventSourceImpl: Spy as unknown as typeof EventSource,
    });
    return { transport, created };
  }

  // Defect A: handshake fails on first try (page opens offline, gateway 502). Old
  // onerror had `if (this.closed || !this.connected) return`, silently dropped this
  // path — never-connected connections never retry.
  it("首次握手就失败时,仍然会重开一条流", async () => {
    vi.useFakeTimers();
    try {
      const { transport, created } = makeWatched();
      transport.connect();
      expect(created).toHaveLength(1);

      created[0].readyState = 2; // CLOSED
      created[0].onerror?.();
      await vi.advanceTimersByTimeAsync(5_000);

      expect(created.length).toBeGreaterThan(1);
    } finally {
      vi.useRealTimers();
    }
  });

  // Defect B: connected once, then gateway kills it. Most likely form on user's server.
  it("连上后进入 CLOSED,会重开一条流并恢复在线状态", async () => {
    vi.useFakeTimers();
    try {
      const status: boolean[] = [];
      const { transport, created } = makeWatched({ onStatus: (v) => status.push(v) });
      transport.connect();
      created[0].onopen?.();
      expect(status).toEqual([true]);

      created[0].readyState = 2; // CLOSED — browser won't retry on its own
      created[0].onerror?.();
      expect(status).toEqual([true, false]);

      await vi.advanceTimersByTimeAsync(5_000);
      expect(created.length).toBeGreaterThan(1);

      created[created.length - 1].onopen?.();
      expect(status[status.length - 1]).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  // Opposite: readyState still CONNECTING, browser **itself** retrying. Open another and
  // we have two parallel streams, server counts an extra consumer, events duplicate.
  it("readyState 仍是 CONNECTING 时不另开流,把重试留给浏览器", async () => {
    vi.useFakeTimers();
    try {
      const { transport, created } = makeWatched();
      transport.connect();
      created[0].onopen?.();

      created[0].readyState = 0; // CONNECTING
      created[0].onerror?.();
      await vi.advanceTimersByTimeAsync(30_000);

      expect(created).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  // close() is user intent (go background, unmount), shouldn't be revived by auto-heal.
  it("close() 之后不再重开", async () => {
    vi.useFakeTimers();
    try {
      const { transport, created } = makeWatched();
      transport.connect();
      created[0].onopen?.();
      transport.close();

      created[0].readyState = 2;
      created[0].onerror?.();
      await vi.advanceTimersByTimeAsync(60_000);

      expect(created).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("HttpTransport 没有的能力", () => {
  // This transport has no push channel. Return false so "More" page hides the toggle;
  // faking success leaves user with switch on but notifications never arriving.
  it("push 订阅诚实地返回 false", () => {
    const t = make();
    expect(t.pushSubscribe({ endpoint: "x" })).toBe(false);
    expect(t.pushUnsubscribe({ endpoint: "x" })).toBe(false);
  });
});

// Device book's "HTTP direct host" cross-origin points to another machine, usually
// with token gate. Two channels must handle token differently, not style: EventSource
// can't set headers.
describe("cross-origin host with a token", () => {
  function makeWithHost(fetchImpl?: typeof fetch) {
    const created: FakeEventSource[] = [];
    class Spy extends FakeEventSource {
      constructor(url: string) {
        super(url);
        created.push(this);
      }
    }
    const transport = new HttpTransport(
      {},
      {
        baseUrl: "https://fleet.example.com",
        token: "tok en/1",
        fetchImpl:
          fetchImpl ??
          (vi.fn(async () => jsonResponse({ ok: true, data: null })) as unknown as typeof fetch),
        eventSourceImpl: Spy as unknown as typeof EventSource,
      },
    );
    return { transport, created };
  }

  it("puts the token in the Authorization header on requests", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ ok: true, data: null }));
    const { transport } = makeWithHost(fetchImpl as unknown as typeof fetch);
    await transport.request("pending_snapshot");
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("https://fleet.example.com/mobile_rpc");
    const headers = init.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer tok en/1");
    expect(headers["Content-Type"]).toBe("application/json");
  });

  // EventSource can't carry headers — server accepts `?token=` for this reason (its
  // comment says "for SSE"). Without this, direct host can't reach stream, symptom is
  // "keeps connecting".
  it("puts the token in the SSE query string, url-encoded", () => {
    const { transport, created } = makeWithHost();
    transport.connect();
    expect(created).toHaveLength(1);
    expect(created[0].url).toBe("https://fleet.example.com/events?token=tok%20en%2F1");
  });

  it("shows the host as the endpoint label", () => {
    const { transport } = makeWithHost();
    expect(transport.endpointLabel).toBe("fleet.example.com");
  });

  it("sends no Authorization header when the host has no token", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ ok: true, data: null }));
    const t = new HttpTransport(
      {},
      {
        baseUrl: "https://open.example.com",
        token: null,
        fetchImpl: fetchImpl as unknown as typeof fetch,
        eventSourceImpl: FakeEventSource as unknown as typeof EventSource,
      },
    );
    await t.request("pending_snapshot");
    const [, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect((init.headers as Record<string, string>).Authorization).toBeUndefined();
  });
});
