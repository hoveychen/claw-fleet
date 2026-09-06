import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// iOS 的 WKWebView 要 iOS 18.4 才有 Screen Wake Lock（16.4–18.3 那版在独立 Web App
// 里根本不工作），所以壳里得有一条原生兜底：@capacitor-community/keep-awake 底下就是
// UIApplication.isIdleTimerDisabled。
//
// 这一层要验的是**接线**而不是原生行为：什么时候装这条兜底、装上之后
// request/release 是不是真转调了原生。原生本身只能真机验。

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
  it("壳里没有标准 API 时装上兜底，持锁真转调原生 keepAwake", async () => {
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { holdWakeLock, isWakeLockSupported } = await import("./wakeLock");
    await installNativeWakeLock();
    // 装上之后设置里的常亮开关才该出现——否则 iOS 用户看到一个消失的开关。
    expect(isWakeLockSupported()).toBe(true);
    const release = holdWakeLock();
    await flush();
    expect(keepAwake).toHaveBeenCalledTimes(1);
    release();
    await flush();
    expect(allowSleep).toHaveBeenCalledTimes(1);
  });

  it("WebView 自带标准 API（iOS 18.4+ / Android）时不装兜底", async () => {
    installEnv({ standardApi: true });
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { holdWakeLock } = await import("./wakeLock");
    await installNativeWakeLock();
    holdWakeLock();
    await flush();
    expect(keepAwake).not.toHaveBeenCalled();
  });

  it("纯浏览器（非壳）里整个不生效", async () => {
    native.value = false;
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { isWakeLockSupported } = await import("./wakeLock");
    await installNativeWakeLock();
    expect(isWakeLockSupported()).toBe(false);
    expect(isSupported).not.toHaveBeenCalled();
  });

  it("原生说不支持时不装——别画一个按下去没反应的开关", async () => {
    isSupported.mockResolvedValue({ isSupported: false });
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { isWakeLockSupported } = await import("./wakeLock");
    await installNativeWakeLock();
    expect(isWakeLockSupported()).toBe(false);
  });

  it("插件抛错（壳里没装好）时静默降级，不炸启动", async () => {
    isSupported.mockRejectedValue(new Error("plugin not implemented"));
    const { installNativeWakeLock } = await import("./wakeLockNative");
    const { isWakeLockSupported } = await import("./wakeLock");
    await expect(installNativeWakeLock()).resolves.toBeUndefined();
    expect(isWakeLockSupported()).toBe(false);
  });
});
