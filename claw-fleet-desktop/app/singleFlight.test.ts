import { beforeEach, describe, expect, it, vi } from "vitest";
import { resetSingleFlight, singleFlight } from "./singleFlight";

beforeEach(() => {
  resetSingleFlight();
});

describe("singleFlight", () => {
  it("在途期间的重复调用只打一次后端", async () => {
    let resolve!: (v: string) => void;
    const run = vi.fn(() => new Promise<string>((r) => (resolve = r)));

    // The startup shape: one timer tick plus a burst of `sessions-updated`.
    const calls = [
      singleFlight("today_usage", run),
      singleFlight("today_usage", run),
      singleFlight("today_usage", run),
    ];
    expect(run).toHaveBeenCalledTimes(1);

    resolve("ok");
    expect(await Promise.all(calls)).toEqual(["ok", "ok", "ok"]);
  });

  it("上一次 settle 之后才放行下一次", async () => {
    const run = vi.fn(() => Promise.resolve(1));

    await singleFlight("k", run);
    await singleFlight("k", run);

    expect(run).toHaveBeenCalledTimes(2);
  });

  it("失败也释放 key,不会把这个 key 卡死", async () => {
    const failing = vi.fn(() => Promise.reject(new Error("backend down")));
    await expect(singleFlight("k", failing)).rejects.toThrow("backend down");

    const ok = vi.fn(() => Promise.resolve("recovered"));
    await expect(singleFlight("k", ok)).resolves.toBe("recovered");
  });

  it("失败会传给每一个等同一次在途请求的调用方", async () => {
    let reject!: (e: Error) => void;
    const run = vi.fn(() => new Promise<string>((_, r) => (reject = r)));

    const first = singleFlight("k", run);
    const second = singleFlight("k", run);
    reject(new Error("boom"));

    await expect(first).rejects.toThrow("boom");
    await expect(second).rejects.toThrow("boom");
  });

  it("不同 key 互不影响", async () => {
    const a = vi.fn(() => new Promise<string>(() => {}));
    const b = vi.fn(() => new Promise<string>(() => {}));

    void singleFlight("live-thinking:s1", a);
    void singleFlight("live-thinking:s2", b);

    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);
  });
});
