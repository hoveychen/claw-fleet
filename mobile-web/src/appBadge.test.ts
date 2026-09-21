import { afterEach, describe, expect, it, vi } from "vitest";
import { canSetAppBadge, setAppBadge } from "./appBadge";

type W = Record<string, unknown>;

afterEach(() => {
  delete (window as unknown as W).fleetNative;
  delete (navigator as unknown as W).setAppBadge;
  delete (navigator as unknown as W).clearAppBadge;
});

/** Stand in for the Badging API, which jsdom does not implement. */
function stubBadging() {
  const set = vi.fn(() => Promise.resolve());
  const clear = vi.fn(() => Promise.resolve());
  (navigator as unknown as W).setAppBadge = set;
  (navigator as unknown as W).clearAppBadge = clear;
  return { set, clear };
}

describe("appBadge", () => {
  it("is a no-op with neither a shell bridge nor the Badging API", () => {
    expect(canSetAppBadge()).toBe(false);
    expect(() => setAppBadge(3)).not.toThrow();
  });

  it("is a no-op on an older shell that lacks the method", () => {
    (window as unknown as W).fleetNative = { scanPairing: () => {} };
    expect(canSetAppBadge()).toBe(false);
    expect(() => setAppBadge(3)).not.toThrow();
  });

  it("falls back to the Badging API in a PWA", () => {
    const { set } = stubBadging();
    expect(canSetAppBadge()).toBe(true);
    setAppBadge(5);
    expect(set).toHaveBeenCalledWith(5);
  });

  it("clears rather than setting zero, which would show a bare dot", () => {
    const { set, clear } = stubBadging();
    setAppBadge(0);
    expect(clear).toHaveBeenCalled();
    expect(set).not.toHaveBeenCalled();
  });

  it("swallows a Badging rejection — an uninstalled PWA is not an error", async () => {
    (navigator as unknown as W).setAppBadge = vi.fn(() => Promise.reject(new Error("nope")));
    expect(() => setAppBadge(2)).not.toThrow();
    await Promise.resolve();
  });

  it("prefers the shell bridge over the Badging API", () => {
    const { set } = stubBadging();
    const setBadge = vi.fn();
    (window as unknown as W).fleetNative = { setBadge };
    setAppBadge(7);
    expect(setBadge).toHaveBeenCalledWith(7);
    expect(set).not.toHaveBeenCalled();
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
