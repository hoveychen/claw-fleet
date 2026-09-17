// Node test environment lacks some browser globals that modules access at import
// time (i18n.ts reads localStorage + navigator.language at top level). Node has
// an experimental localStorage global that reserves the name but is disabled
// (needs --localstorage-file), so here we use defineProperty to forcefully
// override it with a working in-memory implementation so tests that depend on
// it load normally.
const store = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  writable: true,
  value: {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  },
});
if (!("navigator" in globalThis)) {
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    writable: true,
    value: { language: "en" },
  });
}
// i18n.ts also reads window.location.hash at top level (langFromHash), but node
// has no window. Provide minimal shim (location.hash + timers) so modules that
// access window at import time can load. Tests can still override it in
// beforeEach with their own windowShim.
if (!("window" in globalThis)) {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: {
      location: { origin: "http://localhost", hash: "" },
      setTimeout: (fn: () => void, ms?: number) => setTimeout(fn, ms) as unknown as number,
      clearTimeout: (id: number) => clearTimeout(id),
      setInterval: (fn: () => void, ms?: number) => setInterval(fn, ms) as unknown as number,
      clearInterval: (id: number) => clearInterval(id),
    },
  });
}
// Bare global `location` is a separate name the window shim above doesn't reach:
// mock/relay.ts uses `new URLSearchParams(location.search)` at top level to
// detect demo mode, devScrollHarness.tsx reads `location.search` for latency.
// Any test that imports them would ReferenceError: location is not defined at
// import time, unrelated to what the test asserts. Setting search to empty
// string: defaults to "not demo mode", the normal state tests want.
if (!("location" in globalThis)) {
  Object.defineProperty(globalThis, "location", {
    configurable: true,
    writable: true,
    value: { origin: "http://localhost", href: "http://localhost/", hash: "", search: "" },
  });
}
