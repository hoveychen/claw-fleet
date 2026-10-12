import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { enterDemoMode, exitDemoMode, isDemoMode, isMockMode } from "./mockMode";

// Minimal window/localStorage stand-ins: the module only touches location
// (search, hash, reload) and three localStorage calls.
function stubBrowser({ search = "", hash = "" } = {}) {
  const store = new Map<string, string>();
  const location = { search, hash, reload: vi.fn() };
  vi.stubGlobal("window", { location });
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
  return { store, location };
}

describe("demo mode", () => {
  beforeEach(() => vi.unstubAllGlobals());
  afterEach(() => vi.unstubAllGlobals());

  it("is off by default and the ?mock param still turns the mock on", () => {
    stubBrowser();
    expect(isDemoMode()).toBe(false);
    expect(isMockMode()).toBe(false);
    stubBrowser({ search: "?mock" });
    expect(isMockMode()).toBe(true);
  });

  it("entering persists the flag across reloads and turns the mock on", () => {
    const { location } = stubBrowser();
    enterDemoMode();
    expect(location.reload).toHaveBeenCalledOnce();
    expect(isDemoMode()).toBe(true);
    expect(isMockMode()).toBe(true);
  });

  it("a load carrying a real pairing fragment drops the flag", () => {
    const { store, location } = stubBrowser();
    enterDemoMode();
    location.hash = "#k=abc";
    expect(isDemoMode()).toBe(false);
    expect(store.size).toBe(0);
  });

  it("exiting clears the flag and hands a pairing over through the fragment", () => {
    const { location } = stubBrowser();
    enterDemoMode();
    exitDemoMode("#k=abc&relay=https%3A%2F%2Fr.example.com");
    expect(location.hash).toBe("#k=abc&relay=https%3A%2F%2Fr.example.com");
    expect(location.reload).toHaveBeenCalledTimes(2);
    location.hash = "";
    expect(isDemoMode()).toBe(false);
  });
});
