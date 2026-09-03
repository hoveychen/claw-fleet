import { afterEach, describe, expect, it, vi } from "vitest";

// Capacitor 运行时在 node 下不可用，整个替掉；`isNativePlatform` 由每个用例
// 自己拨（模块级 mock 只能装一次，所以经由可变对象透出去）。
const native = { value: false };
vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => native.value },
}));

const { detectVoiceProvider, hasWebSpeech } = await import("./voiceInput");

type Win = Record<string, unknown>;

/** 让 window 看起来像个提供 Web Speech 的浏览器。 */
function giveWebSpeech(): void {
  (window as unknown as Win)["webkitSpeechRecognition"] = function () {};
}

/** 挂上鸿蒙壳的原生桥。`voice` 决定这个壳接没接语音。 */
function giveHarmonyBridge(opts: { voice: boolean }): void {
  const bridge: Win = { scanPairing: () => {} };
  if (opts.voice) bridge["startVoice"] = () => {};
  (window as unknown as Win)["fleetNative"] = bridge;
}

afterEach(() => {
  const w = window as unknown as Win;
  delete w["webkitSpeechRecognition"];
  delete w["SpeechRecognition"];
  delete w["fleetNative"];
  native.value = false;
});

describe("detectVoiceProvider", () => {
  it("浏览器里有 Web Speech 就用它", () => {
    giveWebSpeech();
    expect(detectVoiceProvider()).toBe("web-speech");
  });

  it("浏览器里没有 Web Speech 就判不出来", () => {
    expect(detectVoiceProvider()).toBeNull();
  });

  // 本模块存在的理由。iOS WKWebView 里 Apple 关掉了识别功能却仍然暴露
  // `webkitSpeechRecognition`（WebKit #239816），所以壳里那个对象是个陷阱：
  // 选中它就得到一条 start() 永不出结果、也不报错的死路。判定必须先认壳。
  it("Capacitor 壳里即使 webkitSpeechRecognition 存在也不选 web-speech", () => {
    native.value = true;
    giveWebSpeech();
    expect(detectVoiceProvider()).toBe("capacitor");
  });

  it("鸿蒙桥接了语音就走桥,不看 Capacitor 也不看 Web Speech", () => {
    giveHarmonyBridge({ voice: true });
    native.value = true;
    giveWebSpeech();
    expect(detectVoiceProvider()).toBe("harmony");
  });

  // 老壳配新 web：桥在，但没登记 startVoice。这时候不能选中 harmony —— 那条
  // 实现在壳侧根本不存在，调下去是静默无响应。
  it("鸿蒙桥没接语音时不选 harmony,继续往下回落", () => {
    giveHarmonyBridge({ voice: false });
    giveWebSpeech();
    expect(detectVoiceProvider()).toBe("web-speech");
  });

  it("鸿蒙桥没接语音且没有 Web Speech 时判不出来", () => {
    giveHarmonyBridge({ voice: false });
    expect(detectVoiceProvider()).toBeNull();
  });
});

describe("hasWebSpeech", () => {
  it("认无前缀的 SpeechRecognition", () => {
    (window as unknown as Win)["SpeechRecognition"] = function () {};
    expect(hasWebSpeech()).toBe(true);
  });

  it("两个都没有时为假", () => {
    expect(hasWebSpeech()).toBe(false);
  });
});
