import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { classifyHarmonyError, harmonyVoiceProvider } = await import("./voiceHarmony");

type Win = Record<string, unknown>;

/** Fake native bridge that records what the page calls. */
function installBridge() {
  const calls = { start: [] as string[], stop: 0, cancel: 0 };
  (window as unknown as Win)["fleetNative"] = {
    startVoice: (lang: string) => calls.start.push(lang),
    stopVoice: () => calls.stop++,
    cancelVoice: () => calls.cancel++,
  };
  return calls;
}

/** Simulate pushing an event from the shell side back. */
function push(ev: { kind: string; text?: string; code?: string }): void {
  const hook = (window as unknown as Win)["__fleetVoice"] as
    | ((e: unknown) => void)
    | undefined;
  hook?.({ kind: ev.kind, text: ev.text ?? "", code: ev.code ?? "" });
}

function collect() {
  const partial: string[] = [];
  const final: string[] = [];
  const errors: string[] = [];
  const ready: number[] = [];
  const ended: number[] = [];
  return {
    partial,
    final,
    errors,
    ready,
    ended,
    handlers: {
      onReady: () => ready.push(1),
      onPartial: (t: string) => partial.push(t),
      onFinal: (t: string) => final.push(t),
      onError: (k: string) => errors.push(k),
      onEnd: () => ended.push(1),
    },
  };
}

afterEach(() => {
  const w = window as unknown as Win;
  delete w["fleetNative"];
  delete w["__fleetVoice"];
  delete w["__fleetVoicePermission"];
});

describe("classifyHarmonyError", () => {
  it("认得权限被拒", () => {
    expect(classifyHarmonyError("PERMISSION_DENIED")).toBe("no-permission");
  });

  // Core Speech Kit's numeric error codes have no public enum; they cannot be inferred.
  it("引擎的数字码一律归到 unavailable", () => {
    expect(classifyHarmonyError("1002200002")).toBe("unavailable");
    expect(classifyHarmonyError("START_FAILED")).toBe("unavailable");
  });
});

describe("harmonyVoiceProvider", () => {
  it("桥没接语音时不可用", async () => {
    expect(await harmonyVoiceProvider.isAvailable()).toBe(false);
  });

  it("桥接了语音就可用", async () => {
    installBridge();
    expect(await harmonyVoiceProvider.isAvailable()).toBe(true);
  });

  it("start 把语言传给壳", async () => {
    const calls = installBridge();
    const c = collect();
    await harmonyVoiceProvider.start("zh-CN", c.handlers);
    expect(calls.start).toEqual(["zh-CN"]);
  });

  it("中间结果与定稿分流", async () => {
    installBridge();
    const c = collect();
    await harmonyVoiceProvider.start("zh-CN", c.handlers);
    push({ kind: "partial", text: "把 P3" });
    push({ kind: "final", text: "把 P3 勾掉" });
    expect(c.partial).toEqual(["把 P3"]);
    expect(c.final).toEqual(["把 P3 勾掉"]);
  });

  // After stop, the engine still needs to push back the last final segment. If we
  // tear down the hook on stop, nobody receives it, and the symptom is "finish speaking,
  // press stop, but the last sentence doesn't enter the input box."
  it("stop 之后仍收得到最后一段定稿", async () => {
    const calls = installBridge();
    const c = collect();
    const s = await harmonyVoiceProvider.start("zh-CN", c.handlers);
    s.stop();
    expect(calls.stop).toBe(1);
    push({ kind: "final", text: "最后一句" });
    expect(c.final).toEqual(["最后一句"]);
  });

  it("cancel 之后壳再推什么都不上报", async () => {
    const calls = installBridge();
    const c = collect();
    const s = await harmonyVoiceProvider.start("zh-CN", c.handlers);
    s.cancel();
    expect(calls.cancel).toBe(1);
    push({ kind: "final", text: "不该出现" });
    push({ kind: "error", code: "PERMISSION_DENIED" });
    expect(c.final).toEqual([]);
    expect(c.errors).toEqual([]);
  });

  it("出错后拆掉 hook,后续事件不再上报", async () => {
    installBridge();
    const c = collect();
    await harmonyVoiceProvider.start("zh-CN", c.handlers);
    push({ kind: "error", code: "PERMISSION_DENIED" });
    push({ kind: "final", text: "不该出现" });
    expect(c.errors).toEqual(["no-permission"]);
    expect(c.final).toEqual([]);
  });

  // The engine finishes itself (VAD is done). The final segment has already arrived;
  // here we only tear down the hook and should not report an error.
  it("end 事件收尾,不报错", async () => {
    installBridge();
    const c = collect();
    await harmonyVoiceProvider.start("zh-CN", c.handlers);
    push({ kind: "final", text: "说完了" });
    push({ kind: "end" });
    expect(c.final).toEqual(["说完了"]);
    expect(c.errors).toEqual([]);
    expect((window as unknown as Win)["__fleetVoice"]).toBeUndefined();
  });

  it("桥缺方法时报 unavailable 而不是抛异常", async () => {
    (window as unknown as Win)["fleetNative"] = { scanPairing: () => {} };
    const c = collect();
    const s = await harmonyVoiceProvider.start("zh-CN", c.handlers);
    expect(c.errors).toEqual(["unavailable"]);
    expect(() => {
      s.stop();
      s.cancel();
    }).not.toThrow();
  });
});

// The shell-side createEngine is async; the page must wait until the engine actually
// opens the mic (ready event) before daring to say "listening". When the old shell
// doesn't send this event, we can't hang either — that path is backed up by useVoiceInput
// taking the first partial/final, and here we only guarantee the bridge recognizes this kind.
describe("harmonyVoiceProvider 的就绪信号", () => {
  it("startVoice 之后还没就绪", async () => {
    installBridge();
    const c = collect();
    await harmonyVoiceProvider.start("zh-CN", c.handlers);
    expect(c.ready).toEqual([]);
  });

  it("壳推 ready 才报就绪", async () => {
    installBridge();
    const c = collect();
    await harmonyVoiceProvider.start("zh-CN", c.handlers);
    push({ kind: "ready" });
    expect(c.ready).toEqual([1]);
  });
});

describe("openPermissionSettings", () => {
  // Old shell (no secondary authorization integration) paired with new web: can't pretend
  // we can open the panel, or the UI will draw an "authorize" button that does nothing.
  it("壳没登记 openVoiceSettings 时直接说不行", async () => {
    installBridge();
    expect(await harmonyVoiceProvider.openPermissionSettings?.()).toBe(false);
  });

  it("壳报授权成功就 resolve true 并拆掉 hook", async () => {
    const w = window as unknown as Win;
    let opened = 0;
    w["fleetNative"] = { openVoiceSettings: () => opened++ };

    const pending = harmonyVoiceProvider.openPermissionSettings?.();
    expect(opened).toBe(1);
    (w["__fleetVoicePermission"] as (g: boolean) => void)(true);

    expect(await pending).toBe(true);
    // Clean up: if left behind, the next authorization will hit the same already-resolved promise.
    expect(w["__fleetVoicePermission"]).toBeUndefined();
  });

  it("用户没给就是没给", async () => {
    const w = window as unknown as Win;
    w["fleetNative"] = { openVoiceSettings: () => {} };
    const pending = harmonyVoiceProvider.openPermissionSettings?.();
    (w["__fleetVoicePermission"] as (g: boolean) => void)(false);
    expect(await pending).toBe(false);
  });
});

// HarmonyOS's engine VAD determines silence for 3 seconds and stops itself (or hits the
// 60-second recording limit), then the shell pushes an `end` event. Previously this event
// was only used to tear down the hook, and the page had no idea — the UI kept showing
// "listening", and when the user continued speaking nothing came out; only pressing stop
// again would return.
describe("引擎自己收工", () => {
  it("end 事件要上报给调用方，而不是只在内部拆 hook", async () => {
    installBridge();
    const c = collect();
    await harmonyVoiceProvider.start("zh-CN", c.handlers);
    push({ kind: "ready" });
    push({ kind: "partial", text: "说了半句" });
    push({ kind: "end" });
    expect(c.ended).toHaveLength(1);
  });

  it("用户自己取消之后，壳里补来的 end 不再上报", async () => {
    installBridge();
    const c = collect();
    const session = await harmonyVoiceProvider.start("zh-CN", c.handlers);
    push({ kind: "ready" });
    session.cancel();
    push({ kind: "end" });
    expect(c.ended).toHaveLength(0);
  });

  it("出错收场之后不再补一次 end——一次会话只该有一个结局", async () => {
    installBridge();
    const c = collect();
    await harmonyVoiceProvider.start("zh-CN", c.handlers);
    push({ kind: "error", code: "PERMISSION_DENIED" });
    push({ kind: "end" });
    expect(c.errors).toEqual(["no-permission"]);
    expect(c.ended).toHaveLength(0);
  });
});
