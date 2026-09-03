import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { classifyHarmonyError, harmonyVoiceProvider } = await import("./voiceHarmony");

type Win = Record<string, unknown>;

/** 假的原生桥，记下页面调了什么。 */
function installBridge() {
  const calls = { start: [] as string[], stop: 0, cancel: 0 };
  (window as unknown as Win)["fleetNative"] = {
    startVoice: (lang: string) => calls.start.push(lang),
    stopVoice: () => calls.stop++,
    cancelVoice: () => calls.cancel++,
  };
  return calls;
}

/** 模拟壳侧推一条事件回来。 */
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
  return {
    partial,
    final,
    errors,
    handlers: {
      onPartial: (t: string) => partial.push(t),
      onFinal: (t: string) => final.push(t),
      onError: (k: string) => errors.push(k),
    },
  };
}

afterEach(() => {
  const w = window as unknown as Win;
  delete w["fleetNative"];
  delete w["__fleetVoice"];
});

describe("classifyHarmonyError", () => {
  it("认得权限被拒", () => {
    expect(classifyHarmonyError("PERMISSION_DENIED")).toBe("no-permission");
  });

  // Core Speech Kit 的数字错误码没有公开枚举，猜不得。
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

  // stop 之后引擎还要把最后一段定稿推回来。若 stop 就拆掉 hook，那一段没人收，
  // 现象是「说完按停止，最后一句没进输入框」。
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

  // 引擎自己收工(VAD 说完了)。定稿此前已经到过,这里只拆 hook,不该报错。
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
