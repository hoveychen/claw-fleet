import { afterEach, describe, expect, it, vi } from "vitest";
import { withStallWatch } from "./loadDeadline";

afterEach(() => {
  vi.useRealTimers();
});

describe("withStallWatch", () => {
  it("到期还没 settle 就报一次 stall", async () => {
    vi.useFakeTimers();
    const onStall = vi.fn();
    void withStallWatch(new Promise(() => {}), onStall, 1000);

    await vi.advanceTimersByTimeAsync(999);
    expect(onStall).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(2);
    expect(onStall).toHaveBeenCalledTimes(1);
  });

  it("按时返回的取数不报 stall,值原样透出", async () => {
    vi.useFakeTimers();
    const onStall = vi.fn();
    const out = withStallWatch(Promise.resolve("ok"), onStall, 1000);

    await expect(out).resolves.toBe("ok");
    await vi.advanceTimersByTimeAsync(5000);
    expect(onStall).not.toHaveBeenCalled();
  });

  it("失败的取数也要拆掉定时器,不能在事后补一次假 stall", async () => {
    vi.useFakeTimers();
    const onStall = vi.fn();
    const out = withStallWatch(Promise.reject(new Error("boom")), onStall, 1000);

    await expect(out).rejects.toThrow("boom");
    await vi.advanceTimersByTimeAsync(5000);
    expect(onStall).not.toHaveBeenCalled();
  });
});
