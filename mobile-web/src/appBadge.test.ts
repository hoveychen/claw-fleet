import { afterEach, describe, expect, it, vi } from "vitest";
import { canSetAppBadge, setAppBadge } from "./appBadge";

type W = Record<string, unknown>;

afterEach(() => {
  delete (window as unknown as W).fleetNative;
});

describe("appBadge", () => {
  it("is a no-op without a shell bridge", () => {
    expect(canSetAppBadge()).toBe(false);
    expect(() => setAppBadge(3)).not.toThrow();
  });

  it("is a no-op on an older shell that lacks the method", () => {
    (window as unknown as W).fleetNative = { scanPairing: () => {} };
    expect(canSetAppBadge()).toBe(false);
    expect(() => setAppBadge(3)).not.toThrow();
  });

  it("forwards the count to the shell", () => {
    const setBadge = vi.fn();
    (window as unknown as W).fleetNative = { setBadge };
    setAppBadge(4);
    expect(setBadge).toHaveBeenCalledWith(4);
  });

  it("normalises counts the shell would refuse", () => {
    const setBadge = vi.fn();
    (window as unknown as W).fleetNative = { setBadge };
    setAppBadge(-1);
    setAppBadge(2.7);
    setAppBadge(Number.NaN);
    expect(setBadge.mock.calls.map((c) => c[0])).toEqual([0, 2, 0]);
  });
});
