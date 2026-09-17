// @vitest-environment jsdom
//
// Host-provided UI language default (backend's FLEET_LOCALE, sent via host_features).
//
// Structurally identical to simplified mode's host default, exists for the same reason:
// browser builds store settings in localStorage each, so a host configured with FLEET_LOCALE=zh
// has no way to tell the pages it serves "I'm a Chinese host" — each visitor sees English until
// they navigate to the settings toggle.
//
// Separate file rather than merged into store.test.ts: these tests need to actually load i18n,
// and i18n reads `navigator.language` at init time, so they must run in jsdom (store.test.ts is
// node environment). Four invariants:
//
//   1. Host signals, this client hasn't chosen ⇒ switch language in place, don't wait for next load;
//   2. User explicitly chose ⇒ host cannot override back;
//   3. Cache host's answer for next sync read (i18next's lng must be set synchronously); when host
//      stops signaling, drop cache with it — doesn't become an unrevertible sticky toggle;
//   4. Host signals a language this bundle lacks ⇒ record it but don't switch.
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setTheme: vi.fn(async () => undefined) }),
}));

describe("UI language host default (host_features)", () => {
  beforeEach(() => vi.resetModules());

  it("adopts the host's language in place and caches it for the next load", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, localeDefault: "zh" });

    // Load i18n first: at this point it reads an empty cache, so it falls back to
    // browser language (jsdom reports en-US) — this way the assertion below tests
    // "switched in place", not "happened to read during init".
    const i18n = (await import("./i18n")).default;
    expect(i18n.language).toBe("en");

    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");
    await useUIStore.getState().loadHostFeatures();

    expect(i18n.language).toBe("zh");
    // Cached, not user choice: the latter must still be "hasn't signaled".
    expect(getItem("lang-host-default")).toBe("zh");
    expect(getItem("lang")).toBe(null);
  });

  it("boots straight into the cached host language", async () => {
    const { setItem } = await import("./storage");
    setItem("lang-host-default", "zh");

    const i18n = (await import("./i18n")).default;

    expect(i18n.language).toBe("zh");
  });

  it("lets an explicit choice beat the host default", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, localeDefault: "zh" });

    const { setItem, getItem } = await import("./storage");
    setItem("lang", "en");

    const i18n = (await import("./i18n")).default;
    const { useUIStore } = await import("./store");
    await useUIStore.getState().loadHostFeatures();

    expect(i18n.language).toBe("en");
    // Cache still records host's opinion — so user can reconnect when they clear their own choice later.
    expect(getItem("lang-host-default")).toBe("zh");
  });

  it("drops the cache when the host stops having an opinion", async () => {
    const { setItem, getItem } = await import("./storage");
    setItem("lang-host-default", "zh");

    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false });

    const { useUIStore } = await import("./store");
    await useUIStore.getState().loadHostFeatures();

    expect(getItem("lang-host-default")).toBe(null);
  });

  it("records a language it has no bundle for without switching to it", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, localeDefault: "fr" });

    const i18n = (await import("./i18n")).default;
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");
    await useUIStore.getState().loadHostFeatures();

    expect(i18n.language).toBe("en");
    expect(getItem("lang-host-default")).toBe("fr");
  });
});
