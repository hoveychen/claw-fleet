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

  it("Enable → actually calls navigator.wakeLock.request('screen') and holds sentinel", async () => {
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(1);
    expect(requestMock).toHaveBeenCalledWith("screen");
    expect(localStorage.getItem("fleet-wake-lock")).toBe("1");
  });

  it("Disable → release the held sentinel", async () => {
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    const held = sentinels[0];
    setWakeLockEnabled(false);
    await flush();
    expect(held.release).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem("fleet-wake-lock")).toBe("0");
  });

  it("Repeated enable doesn't re-request (idempotent when lock is held)", async () => {
    const { setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    setWakeLockEnabled(true); // next===enabled, early exit
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(1);
  });

  it("After system releases sentinel in background, foreground returns and re-acquires", async () => {
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

  it("After persist enable, next startup initWakeLock auto-recovers", async () => {
    localStorage.setItem("fleet-wake-lock", "1");
    const { initWakeLock } = await import("./wakeLock");
    initWakeLock();
    await flush();
    expect(requestMock).toHaveBeenCalledWith("screen");
  });

  it("Browser doesn't support wakeLock → supported=false, enable doesn't error or request", async () => {
    installEnv({ supported: false });
    const { isWakeLockSupported, setWakeLockEnabled } = await import("./wakeLock");
    expect(isWakeLockSupported()).toBe(false);
    setWakeLockEnabled(true);
    await flush();
    // Should not throw when wakeLock API is absent (requestMock was never attached)
    expect(localStorage.getItem("fleet-wake-lock")).toBe("1");
  });

  it("Disabled before request returns → immediately release, no ghost lock", async () => {
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
    setWakeLockEnabled(false); // turned off while the await is still pending
    resolveReq(slow); // only now does the request resolve
    await flush();
    expect(slow.release).toHaveBeenCalledTimes(1);
  });
});

// Temporary lock hold during recording: do not change the user's stay-awake setting,
// but as long as anyone is holding it, truly keep the lock. This fixes the issue where
// "screen auto-turns off mid-speech, recognition gets interrupted".
describe("holdWakeLock", () => {
  beforeEach(() => {
    installEnv();
  });

  it("When switch is off, hold still truly requests lock, releases after letting go", async () => {
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

  it("Multiple hold ref counts: when dropping one but one remains, don't release lock", async () => {
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

  it("Same hold repeated release only counts once", async () => {
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

  it("When user switch is on, releasing hold doesn't drop the lock too", async () => {
    const { holdWakeLock, setWakeLockEnabled } = await import("./wakeLock");
    setWakeLockEnabled(true);
    await flush();
    const release = holdWakeLock();
    await flush();
    release();
    await flush();
    expect(sentinels[0].release).not.toHaveBeenCalled();
  });

  it("During hold, system drops lock in background, foreground re-acquires", async () => {
    const { holdWakeLock } = await import("./wakeLock");
    holdWakeLock();
    await flush();
    sentinels[0]._fireRelease();
    for (const fn of visibilityListeners) fn();
    await flush();
    expect(requestMock).toHaveBeenCalledTimes(2);
  });
});
