// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";

import { localCommand } from "./webTransport";

/**
 * `@tauri-apps/plugin-store`'s JS half expects specific shapes back from its
 * commands, not merely "something". Returning the wrong shape doesn't throw —
 * `initStorage()` simply never settles, `boot()` stops before
 * `ReactDOM.render`, and the page stays blank with no console error. That is
 * exactly how it failed the first time, so the contract is pinned here.
 *
 * The shapes are the ones `mock/tauri-mock.ts` documents ("must match expected
 * return types"), which is the only written record of them.
 */
/**
 * This jsdom build exposes `window` but no `localStorage` (verified: it comes
 * back `undefined` even with a real `location.href`), so the suite brings its
 * own minimal one.
 */
function installLocalStorage() {
  const store = new Map<string, string>();
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
      setItem: (k: string, v: string) => void store.set(k, String(v)),
      removeItem: (k: string) => void store.delete(k),
      clear: () => store.clear(),
    },
  });
}

describe("plugin:store contract", () => {
  beforeEach(() => {
    installLocalStorage();
  });

  it("load returns a numeric resource id, not null", () => {
    const { handled, value } = localCommand("plugin:store|load", {});
    expect(handled).toBe(true);
    expect(typeof value).toBe("number");
  });

  it("get returns a [value, exists] tuple", () => {
    localCommand("plugin:store|set", { key: "lang", value: "zh" });

    const hit = localCommand("plugin:store|get", { key: "lang" });
    expect(hit.value).toEqual(["zh", true]);

    // A missing key must report existence as false rather than be absent.
    const miss = localCommand("plugin:store|get", { key: "nope" });
    expect(miss.value).toEqual([null, false]);
  });

  it("set then delete round-trips through localStorage", () => {
    localCommand("plugin:store|set", { key: "viewMode", value: "gallery" });
    expect(localCommand("plugin:store|get", { key: "viewMode" }).value).toEqual([
      "gallery",
      true,
    ]);

    localCommand("plugin:store|delete", { key: "viewMode" });
    expect(localCommand("plugin:store|get", { key: "viewMode" }).value).toEqual([
      null,
      false,
    ]);
  });

  it("collection and scalar queries return their own types", () => {
    // Each of these is destructured or compared by the plugin; a blanket null
    // would break the caller in a different way for each one.
    expect(localCommand("plugin:store|entries", {}).value).toEqual([]);
    expect(localCommand("plugin:store|keys", {}).value).toEqual([]);
    expect(localCommand("plugin:store|values", {}).value).toEqual([]);
    expect(localCommand("plugin:store|length", {}).value).toBe(0);
    expect(localCommand("plugin:store|has", { key: "x" }).value).toBe(false);
  });

  it("keeps its own namespace so mock fixtures cannot bleed in", () => {
    window.localStorage.setItem("mock-store:lang", "en");
    localCommand("plugin:store|set", { key: "lang", value: "zh" });
    expect(window.localStorage.getItem("mock-store:lang")).toBe("en");
    expect(localCommand("plugin:store|get", { key: "lang" }).value).toEqual(["zh", true]);
  });
});

describe("host facts", () => {
  it("reports the browser build honestly instead of impersonating a platform", () => {
    expect(localCommand("get_platform", {}).value).toBe("web");
  });

  it("leaves unknown commands unhandled so the caller can record the gap", () => {
    expect(localCommand("open_settings_window", {}).handled).toBe(false);
  });
});

/**
 * A blanket `null` for unmapped commands is only safe for callers that ignore
 * the result. Anything a component reads `.length` off during render must come
 * back as a list, or React throws mid-render and the whole tree unmounts to an
 * empty root — with `boot()` still reporting success, which is exactly how this
 * first showed up.
 *
 * `ConnectionDialog` does `conns.length === 0` on the value of
 * `list_saved_connections`, so these two are load-bearing.
 */
describe("list-shaped commands never answer null", () => {
  it.each(["list_saved_connections", "list_ssh_profiles"])("%s returns an array", (cmd) => {
    const { handled, value } = localCommand(cmd, {});
    expect(handled).toBe(true);
    expect(Array.isArray(value)).toBe(true);
  });
});
