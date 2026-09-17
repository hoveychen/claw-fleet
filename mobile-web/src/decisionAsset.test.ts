import { describe, expect, it } from "vitest";
import { fetchDecisionAsset } from "./decisionAsset";
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
