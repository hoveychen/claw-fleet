import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// iOS's WKWebView only has Screen Wake Lock on iOS 18.4 (16.4–18.3 doesn't work in standalone Web App),
// so the shell needs a native fallback: @capacitor-community/keep-awake wraps UIApplication.isIdleTimerDisabled.
//
// What we test here is **wiring**, not native behavior: when to install the fallback, whether
// request/release actually delegates to native after install. Native behavior itself only testable on device.

const native = { value: true };
const keepAwake = vi.fn(async () => {});
const allowSleep = vi.fn(async () => {});
const isSupported = vi.fn(async () => ({ isSupported: true }));

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => native.value },
}));
vi.mock("@capacitor-community/keep-awake", () => ({
  KeepAwake: { keepAwake, allowSleep, isSupported },
}));

function installEnv(opts: { standardApi?: boolean } = {}) {
  const nav: Record<string, unknown> = { language: "en" };
  if (opts.standardApi) nav.wakeLock = { request: vi.fn(async () => ({})) };
  Object.defineProperty(globalThis, "navigator", { configurable: true, writable: true, value: nav });
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    writable: true,
    value: { visibilityState: "visible", addEventListener: () => {} },
  });
  localStorage.clear();
}

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  native.value = true;
  keepAwake.mockClear();
  allowSleep.mockClear();
  isSupported.mockClear();
  isSupported.mockResolvedValue({ isSupported: true });
  installEnv();
});

afterEach(() => {
  vi.resetModules();
});

describe("installNativeWakeLock", () => {
  it("installs fallback when shell lacks standard API, lock actually delegates to native keepAwake", async () => {
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { holdWakeLock, isWakeLockSupported } = await import("./wakeLock");
    await installNativeWakeLock();
    // After install, always-on toggle should appear in settings — else iOS user sees a phantom toggle.
    expect(isWakeLockSupported()).toBe(true);
    const release = holdWakeLock();
    await flush();
    expect(keepAwake).toHaveBeenCalledTimes(1);
    release();
    await flush();
    expect(allowSleep).toHaveBeenCalledTimes(1);
  });

  it("does not install fallback when WebView has standard API (iOS 18.4+ / Android)", async () => {
    installEnv({ standardApi: true });
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { holdWakeLock } = await import("./wakeLock");
    await installNativeWakeLock();
    holdWakeLock();
    await flush();
    expect(keepAwake).not.toHaveBeenCalled();
  });

  it("has no effect in pure browser (not shell)", async () => {
    native.value = false;
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { isWakeLockSupported } = await import("./wakeLock");
    await installNativeWakeLock();
    expect(isWakeLockSupported()).toBe(false);
    expect(isSupported).not.toHaveBeenCalled();
  });

  it("does not install when native says unsupported — don't draw a dead button", async () => {
    isSupported.mockResolvedValue({ isSupported: false });
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { isWakeLockSupported } = await import("./wakeLock");
    await installNativeWakeLock();
    expect(isWakeLockSupported()).toBe(false);
  });

  it("silently degrades when plugin throws (shell not installed correctly), does not break startup", async () => {
    isSupported.mockRejectedValue(new Error("plugin not implemented"));
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { isWakeLockSupported } = await import("./wakeLock");
    await expect(installNativeWakeLock()).resolves.toBeUndefined();
    expect(isWakeLockSupported()).toBe(false);
  });
});
