import { beforeEach, describe, expect, it, vi } from "vitest";

// 假插件：既供错误码分类那几个纯用例（它们碰不到这里），也让「一次识别的收场」
// 可以被真的驱动一遍 —— 原生插件在 node 下跑不了，但它与我们之间的那层约定
// （listener、start 的 promise 什么时候 resolve）是可以受控重放的。
const listeners: Record<string, (e: unknown) => void> = {};
let resolveStart: ((r: { matches?: string[] }) => void) | undefined;
let rejectStart: ((e: unknown) => void) | undefined;

vi.mock("@capgo/capacitor-speech-recognition", () => ({
  SpeechRecognition: {
    checkPermissions: async () => ({ speechRecognition: "granted" }),
    requestPermissions: async () => ({ speechRecognition: "granted" }),
    addListener: async (name: string, cb: (e: unknown) => void) => {
      listeners[name] = cb;
      return { remove: () => delete listeners[name] };
    },
    start: () =>
      new Promise<{ matches?: string[] }>((res, rej) => {
        resolveStart = res;
        rejectStart = rej;
      }),
    stop: async () => {},
  },
}));

const { classifyNativeError, capacitorVoiceProvider } = await import("./voiceCapacitor");

function collect() {
  const final: string[] = [];
  const errors: string[] = [];
  const ended: number[] = [];
  return {
    final,
    errors,
    ended,
    handlers: {
      onReady: () => {},
      onPartial: () => {},
      onFinal: (t: string) => final.push(t),
      onError: (k: string) => errors.push(k),
      onEnd: () => ended.push(1),
    },
  };
}

beforeEach(() => {
  for (const k of Object.keys(listeners)) delete listeners[k];
  resolveStart = undefined;
  rejectStart = undefined;
});

describe("classifyNativeError", () => {
  it("认得权限类", () => {
    expect(classifyNativeError("ERROR_INSUFFICIENT_PERMISSIONS")).toBe("no-permission");
    expect(classifyNativeError("permission_denied")).toBe("no-permission");
    expect(classifyNativeError("not-allowed")).toBe("no-permission");
  });

  it("认得没听清", () => {
    expect(classifyNativeError("ERROR_NO_MATCH")).toBe("no-speech");
    expect(classifyNativeError("ERROR_SPEECH_TIMEOUT")).toBe("no-speech");
  });

  it("认得网络类", () => {
    expect(classifyNativeError("ERROR_NETWORK")).toBe("network");
    expect(classifyNativeError("ERROR_NETWORK_TIMEOUT")).toBe("network");
    expect(classifyNativeError("ERROR_SERVER")).toBe("network");
  });

  it("认得服务不可用", () => {
    expect(classifyNativeError("ON_DEVICE_RECOGNITION_UNAVAILABLE")).toBe("unavailable");
  });

  // 两个平台的码值各说各话且没有文档化枚举，所以认不出是常态。宁可笼统说
  // 「没有可用的语音识别」，也不要把网络问题说成没有权限、让用户白跑一趟设置。
  it("认不出的一律归到 unavailable,不乱猜", () => {
    expect(classifyNativeError("ERROR_CLIENT")).toBe("unavailable");
    expect(classifyNativeError("")).toBe("unavailable");
    expect(classifyNativeError("某个没见过的码")).toBe("unavailable");
  });

  it("大小写不敏感", () => {
    expect(classifyNativeError("error_network")).toBe("network");
    expect(classifyNativeError("ERROR_NETWORK")).toBe("network");
  });
});

// 插件的 start() 要到**整段识别结束**才 resolve —— 那一刻就是「引擎收工了」，
// 不管是用户按的停止还是原生自己判定说完了。以前这里只把最终结果报上去，没有
// 说会话已经结束，于是界面继续显示「正在听」。
describe("一次识别的收场", () => {
  it("原生收工时上报 onEnd，最终结果照旧先报", async () => {
    const c = collect();
    await capacitorVoiceProvider.start("zh-CN", c.handlers);
    resolveStart!({ matches: ["说完了"] });
    await Promise.resolve();
    await Promise.resolve();
    expect(c.final).toEqual(["说完了"]);
    expect(c.ended).toHaveLength(1);
  });

  it("用户取消之后，原生补来的收场不上报", async () => {
    const c = collect();
    const session = await capacitorVoiceProvider.start("zh-CN", c.handlers);
    session.cancel();
    resolveStart!({ matches: ["不要这句"] });
    await Promise.resolve();
    await Promise.resolve();
    expect(c.final).toEqual([]);
    expect(c.ended).toHaveLength(0);
  });

  it("报错收场只报错，不再补一次 onEnd", async () => {
    const c = collect();
    await capacitorVoiceProvider.start("zh-CN", c.handlers);
    rejectStart!(new Error("ERROR_NETWORK"));
    await Promise.resolve();
    await Promise.resolve();
    expect(c.errors).toEqual(["network"]);
    expect(c.ended).toHaveLength(0);
  });
});
