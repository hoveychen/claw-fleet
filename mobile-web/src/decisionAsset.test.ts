import { describe, expect, it } from "vitest";
import { fetchDecisionAsset } from "./decisionAsset";
import type { RelayClient } from "./relay";
import { fetchWikiFile } from "./wiki";

// relay.ts REQUEST_TIMEOUT_MS 默认 15000。asset/upload 走这个默认值时，慢网下
// MB 级 base64 传不完就在 15s 早退：pending 被删 → 迟到 reply 被丢 → 决策卡的
// <img> 静默卡死（浏览器 e2e 已复现，agent 延迟 20s 返回、图永不出、console 0 报错）。
// 这些资源类请求必须传一个远大于 15s 的窗口。
const DEFAULT_CONTROL_TIMEOUT_MS = 15_000;

/** 捕获 client.request 收到的 (method, timeoutMs) 的假 client。 */
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

describe("资源/上传类请求用加长超时（防慢网 15s 静默早退）", () => {
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
