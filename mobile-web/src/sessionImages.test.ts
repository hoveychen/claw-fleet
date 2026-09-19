import { describe, expect, it } from "vitest";
import type { RelayClient } from "./relay";
import { fetchSessionImage, fetchSessionImages } from "./sessionImages";

const DEFAULT_CONTROL_TIMEOUT_MS = 15_000;

/** Mock client capturing (method, params, timeoutMs) and returning a canned
 *  reply shaped like the relay's. */
function captor(reply: unknown) {
  const calls: Array<{
    method: string;
    params?: unknown;
    timeoutMs?: number;
  }> = [];
  const client = {
    request: (method: string, params?: unknown, timeoutMs?: number) => {
      calls.push({ method, params, timeoutMs });
      return Promise.resolve(reply);
    },
  } as unknown as RelayClient;
  return { client, calls };
}

describe("生成图片的取数", () => {
  it("列表返回文件名与字节数", async () => {
    const { client, calls } = captor({
      images: [{ name: "1.png", bytes: 42 }],
    });
    const images = await fetchSessionImages(client, "img-abc");
    expect(calls[0].method).toBe("session_images");
    expect(calls[0].params).toEqual({ session: "img-abc" });
    expect(images).toEqual([{ name: "1.png", bytes: 42 }]);
  });

  it("列表为空或缺字段时给空数组而不是抛错", async () => {
    // An agent on an older build answers without the field; the strip should
    // render as empty rather than crashing the whole transcript view.
    const { client } = captor({});
    expect(await fetchSessionImages(client, "img-abc")).toEqual([]);
    const nullish = captor(null);
    expect(await fetchSessionImages(nullish.client, "img-abc")).toEqual([]);
  });

  it("取字节用的超时远大于 15s 控制消息默认值", async () => {
    // Same pitfall as decision_asset: a downscaled 4K render is still MB-scale,
    // and a spurious 15s abort strands the <img> forever.
    const { client, calls } = captor({ mime: "image/png", base64: "" });
    await fetchSessionImage(client, "img-abc", "1.png");
    expect(calls[0].method).toBe("session_image");
    expect(calls[0].params).toEqual({ session: "img-abc", name: "1.png" });
    expect(calls[0].timeoutMs ?? 0).toBeGreaterThan(DEFAULT_CONTROL_TIMEOUT_MS);
  });
});
