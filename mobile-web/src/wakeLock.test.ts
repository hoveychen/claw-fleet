import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Controlled verification of screen stay-awake core behavior: on → truly calls
// navigator.wakeLock.request("screen"), off → release, return to foreground and re-acquire.
// Node has no world isolation (patchwright eval's isolated world prevents injected spies from
// reaching the module's objects, so we can't use browser eval to verify this layer).
//
// Module reads document / navigator at import time, so each test case sets up mocks first,
// then dynamic import, and uses vi.resetModules() to prevent module-level enabled/sentinel state leakage.

type Sentinel = {
  released: boolean;
  release: ReturnType<typeof vi.fn>;
  addEventListener: (t: "release", fn: () => void) => void;
  _fireRelease: () => void;
};

function makeSentinel(): Sentinel {
  let onRelease: (() => void) | null = null;
  const s: Sentinel = {
    released: false,
    release: vi.fn(async () => {
      s.released = true;
    }),
    addEventListener: (_t, fn) => {
      onRelease = fn;
    },
    _fireRelease: () => {
      s.released = true;
      onRelease?.();
    },
  };
  return s;
}

let requestMock: ReturnType<typeof vi.fn>;
let sentinels: Sentinel[];
let visibilityListeners: Array<() => void>;

function installEnv(opts: { supported?: boolean; visible?: boolean } = {}) {
  const { supported = true, visible = true } = opts;
  sentinels = [];
  visibilityListeners = [];
  requestMock = vi.fn(async () => {
    const s = makeSentinel();
    sentinels.push(s);
    return s;
  });

  const nav: Record<string, unknown> = { language: "en" };
  if (supported) nav.wakeLock = { request: requestMock };
  Object.defineProperty(globalThis, "navigator", { configurable: true, writable: true, value: nav });

  Object.defineProperty(globalThis, "document", {
    configurable: true,
    writable: true,
    value: {
      visibilityState: visible ? "visible" : "hidden",
      addEventListener: (t: string, fn: () => void) => {
        if (t === "visibilitychange") visibilityListeners.push(fn);
      },
    },
  });

  localStorage.clear();
}

// Let internal await microtasks in acquire()/drop() finish
const flush = () => new Promise((r) => setTimeout(r, 0));

afterEach(() => {
  vi.resetModules();
});

describe("wakeLock", () => {
  beforeEach(() => {
    installEnv();
  });

  it("开启 → 真调 navigator.wakeLock.request('screen') 并持有 sentinel", async () => {
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(1);
    expect(requestMock).toHaveBeenCalledWith("screen");
    expect(localStorage.getItem("fleet-wake-lock")).toBe("1");
  });

  it("关闭 → release 已持有的 sentinel", async () => {
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    const held = sentinels[0];
    setWakeLockEnabled(false);
    await flush();
    expect(held.release).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem("fleet-wake-lock")).toBe("0");
  });

  it("重复开启不重复申请（已持锁时幂等）", async () => {
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    setWakeLockEnabled(true); // next===enabled, early exit
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(1);
  });

  it("系统在后台释放 sentinel 后，回前台重新 acquire", async () => {
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    // Simulate system auto-release (background / low battery)
    sentinels[0]._fireRelease();
    // Return to foreground
    for (const fn of visibilityListeners) fn();
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(2);
  });

  it("持久化开启后，下次启动 initWakeLock 自动恢复", async () => {
    localStorage.setItem("fleet-wake-lock", "1");
    const { initWakeLock } = await import("./wakeLock");
    initWakeLock();
    await flush();
    expect(requestMock).toHaveBeenCalledWith("screen");
  });

  it("浏览器不支持 wakeLock → supported=false，开启不报错也不申请", async () => {
    installEnv({ supported: false });
    const { isWakeLockSupported, setWakeLockEnabled } = await import("./wakeLock");
    expect(isWakeLockSupported()).toBe(false);
    setWakeLockEnabled(true);
    await flush();
    // Should not throw when wakeLock API is absent (requestMock was never attached)
    expect(localStorage.getItem("fleet-wake-lock")).toBe("1");
  });

  it("请求返回前被关掉 → 立即释放，不留幽灵锁", async () => {
    // Make request return slowly, turn off the switch in the meantime
    let resolveReq!: (s: Sentinel) => void;
    const slow = makeSentinel();
    requestMock.mockImplementationOnce(
      () =>
        new Promise<Sentinel>((res) => {
          resolveReq = res;
        }),
    );
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    setWakeLockEnabled(false); // await 期间关掉
    resolveReq(slow); // 现在 request 才 resolve
    await flush();
    expect(slow.release).toHaveBeenCalledTimes(1);
  });
});

// Temporary lock hold during recording: do not change the user's stay-awake setting,
// but as long as anyone is holding it, truly keep the lock. This is the fix for
// "screen auto-turns off mid-speech, recognition gets interrupted".
describe("holdWakeLock", () => {
  beforeEach(() => {
    installEnv();
  });

  it("开关关着时 hold 也真申请锁，释放后放掉", async () => {
    const { holdWakeLock, getWakeLockEnabled } = await import("./wakeLock");
    const release = holdWakeLock();
    await flush();
    expect(requestMock).toHaveBeenCalledWith("screen");
    // Temporary hold should not flip the user's switch — after recording, return to their setting.
    expect(getWakeLockEnabled()).toBe(false);
    release();
    await flush();
    expect(sentinels[0].release).toHaveBeenCalledTimes(1);
  });

  it("多个 hold 引用计数：放掉一个还剩一个时不松锁", async () => {
    const { holdWakeLock } = await import("./wakeLock");
    const a = holdWakeLock();
    const b = holdWakeLock();
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(1);
    a();
    await flush();
    expect(sentinels[0].release).not.toHaveBeenCalled();
    b();
    await flush();
    expect(sentinels[0].release).toHaveBeenCalledTimes(1);
  });

  it("同一个 hold 重复释放只算一次", async () => {
    const { holdWakeLock } = await import("./wakeLock");
    const a = holdWakeLock();
    const b = holdWakeLock();
    await flush();
    a();
    a();
    await flush();
    // Second a() call should not cancel out b's hold.
    expect(sentinels[0].release).not.toHaveBeenCalled();
    b();
    await flush();
    expect(sentinels[0].release).toHaveBeenCalledTimes(1);
  });

  it("用户开关开着时，释放 hold 不会把锁一起放掉", async () => {
    const { holdWakeLock, setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    const release = holdWakeLock();
    await flush();
    release();
    await flush();
    expect(sentinels[0].release).not.toHaveBeenCalled();
  });

  it("hold 期间系统在后台放掉了锁，回前台重新拿", async () => {
    const { holdWakeLock } = await import("./wakeLock");
    holdWakeLock();
    await flush();
    sentinels[0]._fireRelease();
    for (const fn of visibilityListeners) fn();
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(2);
  });
});
