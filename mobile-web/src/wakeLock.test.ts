import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// 受控验证屏幕常亮的核心行为：开→acquire 真调 navigator.wakeLock.request("screen")，
// 关→release，回前台重拿。node 环境无 world 隔离（patchwright eval 的隔离 world 会
// 让注入的 spy 打不到模块看到的对象，故不能用浏览器 eval 验证这一层）。
//
// 模块在 import 期会读 document / navigator，所以每个用例先装好 mock 再动态 import，
// 并用 vi.resetModules() 保证模块级 enabled/sentinel 状态互不串扰。

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

// 让 acquire()/drop() 内部的 await 微任务跑完
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
    setWakeLockEnabled(true); // next===enabled，早退
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(1);
  });

  it("系统在后台释放 sentinel 后，回前台重新 acquire", async () => {
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    // 模拟系统自动释放（切后台 / 低电量）
    sentinels[0]._fireRelease();
    // 回前台
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
    // 没有 wakeLock API 时不应抛错（requestMock 根本没挂上去）
    expect(localStorage.getItem("fleet-wake-lock")).toBe("1");
  });

  it("请求返回前被关掉 → 立即释放，不留幽灵锁", async () => {
    // 让 request 慢一拍返回，期间关掉开关
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

// 录音期间的临时持锁：不改用户的常亮开关，但只要还有人 hold 着，就得真持着锁。
// 这是「说到一半手机自动息屏，识别被打断」的修复面。
describe("holdWakeLock", () => {
  beforeEach(() => {
    installEnv();
  });

  it("开关关着时 hold 也真申请锁，释放后放掉", async () => {
    const { holdWakeLock, getWakeLockEnabled } = await import("./wakeLock");
    const release = holdWakeLock();
    await flush();
    expect(requestMock).toHaveBeenCalledWith("screen");
    // 临时持锁不该把用户的开关掰开——录完要回到他自己的设置。
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
    // 第二次 a() 不该把 b 的那一份也抵消掉。
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
