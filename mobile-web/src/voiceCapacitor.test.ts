import { describe, expect, it, vi } from "vitest";

// 原生插件在 node 下不可用，且本测试只针对错误码分类这段纯逻辑。
vi.mock("@capgo/capacitor-speech-recognition", () => ({
  SpeechRecognition: {},
}));

const { classifyNativeError } = await import("./voiceCapacitor");

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
