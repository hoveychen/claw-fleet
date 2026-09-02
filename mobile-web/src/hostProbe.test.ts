import { describe, expect, it, vi } from "vitest";
import { probeHost, probeMessage } from "./hostProbe";

function jsonRes(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  } as unknown as Response;
}

const OK_FETCH = vi.fn(async () => jsonRes({ ok: true, data: [] }));

describe("probeHost", () => {
  it("accepts a host that answers the mobile RPC envelope", async () => {
    const fetchImpl = vi.fn(async () => jsonRes({ ok: true, data: [] }));
    const r = await probeHost("https://fleet.example.com/", "tok", {
      fetchImpl: fetchImpl as unknown as typeof fetch,
      pageProtocol: "https:",
    });
    expect(r.ok).toBe(true);
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    // 末尾斜杠被规范掉,否则会打到 `//mobile_rpc`。
    expect(url).toBe("https://fleet.example.com/mobile_rpc");
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer tok");
  });

  it("rejects a non-url before touching the network", async () => {
    const fetchImpl = vi.fn();
    const r = await probeHost("fleet.example.com", null, {
      fetchImpl: fetchImpl as unknown as typeof fetch,
      pageProtocol: "https:",
    });
    expect(r).toEqual({ ok: false, reason: "not-a-url" });
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  // 混合内容是浏览器的硬规则,探测时就该说清楚 —— 否则用户会以为是 token 错。
  it("names mixed content instead of letting the browser eat it", async () => {
    const fetchImpl = vi.fn();
    const r = await probeHost("http://192.168.1.5:8080", null, {
      fetchImpl: fetchImpl as unknown as typeof fetch,
      pageProtocol: "https:",
    });
    expect(r).toEqual({ ok: false, reason: "mixed-content" });
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("allows http on localhost, and any http when the page is http", async () => {
    const local = await probeHost("http://localhost:8080", null, {
      fetchImpl: OK_FETCH as unknown as typeof fetch,
      pageProtocol: "https:",
    });
    expect(local.ok).toBe(true);
    const devPage = await probeHost("http://192.168.1.5:8080", null, {
      fetchImpl: OK_FETCH as unknown as typeof fetch,
      pageProtocol: "http:",
    });
    expect(devPage.ok).toBe(true);
  });

  // 跨源被拒与网络不通在 JS 里长得一样(浏览器刻意不交出原因),所以这一档只能
  // 如实合并,不能假装分得清。
  it("folds a CORS rejection into unreachable, with the raw detail kept", async () => {
    const fetchImpl = vi.fn(async () => {
      throw new TypeError("Failed to fetch");
    });
    const r = await probeHost("https://fleet.example.com", null, {
      fetchImpl: fetchImpl as unknown as typeof fetch,
      pageProtocol: "https:",
    });
    expect(r).toMatchObject({ ok: false, reason: "unreachable" });
    if (r.ok || r.reason !== "unreachable") throw new Error("expected unreachable");
    expect(r.detail).toContain("Failed to fetch");
  });

  it("tells a wrong token apart from a wrong address", async () => {
    for (const status of [401, 403]) {
      const r = await probeHost("https://fleet.example.com", "bad", {
        fetchImpl: (vi.fn(async () => jsonRes({}, status)) as unknown) as typeof fetch,
        pageProtocol: "https:",
      });
      expect(r).toEqual({ ok: false, reason: "unauthorized", status });
    }
  });

  it("rejects a 200 that is not the mobile RPC envelope", async () => {
    const r = await probeHost("https://not-fleet.example.com", null, {
      fetchImpl: (vi.fn(async () => jsonRes({ hello: "world" })) as unknown) as typeof fetch,
      pageProtocol: "https:",
    });
    expect(r).toMatchObject({ ok: false, reason: "not-fleet" });
  });

  it("rejects a 404 — that address is not a Fleet host", async () => {
    const r = await probeHost("https://example.com", null, {
      fetchImpl: (vi.fn(async () => jsonRes({}, 404)) as unknown) as typeof fetch,
      pageProtocol: "https:",
    });
    expect(r).toEqual({ ok: false, reason: "not-fleet", status: 404 });
  });
});

describe("probeMessage", () => {
  it("has a distinct, actionable line for every failure", () => {
    const seen = new Set<string>();
    const results = [
      { ok: false as const, reason: "not-a-url" as const },
      { ok: false as const, reason: "mixed-content" as const },
      { ok: false as const, reason: "unreachable" as const, detail: "x" },
      { ok: false as const, reason: "unauthorized" as const, status: 401 },
      { ok: false as const, reason: "not-fleet" as const, status: 404 },
    ];
    for (const r of results) {
      const msg = probeMessage(r);
      expect(msg.length).toBeGreaterThan(0);
      // 每一种失败都要有自己的话:合并成一句「添加失败」就等于让用户去猜。
      expect(seen.has(msg)).toBe(false);
      seen.add(msg);
    }
    expect(probeMessage({ ok: true })).toBe("");
  });
});
