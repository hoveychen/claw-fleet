import { describe, expect, it } from "vitest";
import { fetchDecisionAsset, inlineImgSrcs } from "./decisionAsset";
import type { RelayClient } from "./relay";
import { fetchWikiFile } from "./wiki";

// relay.ts REQUEST_TIMEOUT_MS defaults to 15000. When asset/upload uses this
// default over slow networks, megabyte-scale base64 doesn't finish before the
// 15s timeout fires: pending gets deleted → late reply is dropped → decision
// card <img> hangs silently (browser e2e reproduced: agent returns after 20s,
// image never appears, console silent). Resource requests like this need a
// window much larger than 15s.
const DEFAULT_CONTROL_TIMEOUT_MS = 15_000;

/** Mock client that captures (method, timeoutMs) pairs passed to
 *  client.request(). */
function captor() {
  const calls: Array<{ method: string; timeoutMs?: number }> = [];
  const client = {
    request: (method: string, _params?: unknown, timeoutMs?: number) => {
      calls.push({ method, timeoutMs });
      return Promise.resolve({ mime: "image/png", base64: "" });
    },
  } as unknown as RelayClient;
  return { client, calls };
}

describe("Asset/upload requests use extended timeout (prevent silent 15s timeout on slow networks)", () => {
  it("decision_asset 的超时远大于 15s 控制消息默认值", async () => {
    const { client, calls } = captor();
    await fetchDecisionAsset(client, "ask-img", 0, "chart.png");
    expect(calls[0].method).toBe("decision_asset");
    expect(calls[0].timeoutMs ?? 0).toBeGreaterThan(DEFAULT_CONTROL_TIMEOUT_MS);
  });

  it("wiki_file 的超时远大于 15s 控制消息默认值", async () => {
    const { client, calls } = captor();
    await fetchWikiFile(client, "slug", "20260101-000000", "index.html");
    expect(calls[0].method).toBe("wiki_file");
    expect(calls[0].timeoutMs ?? 0).toBeGreaterThan(DEFAULT_CONTROL_TIMEOUT_MS);
  });
});

describe("inlineImgSrcs", () => {
  const uris = new Map([
    ["a.png", "data:image/png;base64,A"],
    ["d-cold.png", "data:image/png;base64,D"],
  ]);

  it("rewrites double-quoted, single-quoted and unquoted refs", () => {
    const html = `<img src="a.png" alt="x"><img src='a.png'><img src=d-cold.png><img alt=y src=d-cold.png />`;
    const out = inlineImgSrcs(html, uris);
    expect(out).toContain('<img src="data:image/png;base64,A" alt="x">');
    expect(out).toContain("<img src='data:image/png;base64,A'>");
    expect(out).toContain('<img src="data:image/png;base64,D">');
    expect(out).toContain('alt=y src="data:image/png;base64,D" />');
  });

  it("leaves refs it has no URI for untouched", () => {
    const html = `<img src=missing.png><img src="https://cdn/c.png">`;
    expect(inlineImgSrcs(html, uris)).toBe(html);
  });
});
