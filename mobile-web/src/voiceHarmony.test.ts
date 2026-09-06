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

// 壳侧的 createEngine 是异步的，页面必须等到引擎真的开麦（ready 事件）才敢说
// 「正在听」。老壳不发这个事件时也不能卡死——那一路由 useVoiceInput 拿首个
// partial/final 兜底，这里只保证桥认得这个 kind。
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
  // 老壳（没接二次授权）配新 web：不能假装能拉起面板，否则 UI 会画一个按下去
  // 什么都不发生的「去授权」。
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
    // 拆干净:留着的话下一次授权会撞上同一个已经 resolve 过的 promise。
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

// 鸿蒙的引擎 VAD 判定静默 3 秒就自己收工（也可能是录满 60 秒的上限），壳侧推一条
// `end` 回来。以前这条事件只用来拆 hook，页面完全不知道 —— 界面一直显示「正在
// 听」，用户继续说却一个字都不出，只有再点一次停止才回得来。
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
