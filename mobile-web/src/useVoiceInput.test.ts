import { describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { voiceErrorHint, voiceErrorText } = await import("./useVoiceInput");

describe("voiceErrorText", () => {
  // 「请在系统设置里允许」那半句挪进了 hint —— 标题里再留着,壳里就会和旁边的
  // 「去授权」按钮打架,浏览器里则指向一个根本不存在的地方。
  it("没权限的标题里不再自带指引", () => {
    const text = voiceErrorText("no-permission");
    expect(text).not.toContain("系统设置");
    expect(text).toBeTruthy();
  });
});

describe("voiceErrorHint", () => {
  // 能把用户送过去时,那句话是多余的:按钮本身就是指引。
  it("能拉起授权面板时不给指引文字", () => {
    expect(voiceErrorHint("no-permission", true, "harmony")).toBeNull();
  });

  // 浏览器里的麦克风权限在**站点设置**里,不在系统设置里。指错地方比不指更糟:
  // 用户会真的去翻一遍系统设置,回来发现没用。
  it("浏览器里指向站点设置而不是系统设置", () => {
    const hint = voiceErrorHint("no-permission", false, "web-speech");
    expect(hint).toContain("站点");
    expect(hint).not.toContain("系统设置");
  });

  it("壳里送不过去时才指向系统设置", () => {
    const hint = voiceErrorHint("no-permission", false, "capacitor");
    expect(hint).toContain("系统设置");
  });

  // 这两类跟权限无关,指引不该跟着权限的分支走。
  it("网络与不可用各有自己的下一步", () => {
    expect(voiceErrorHint("network", true, "web-speech")).toBeTruthy();
    expect(voiceErrorHint("unavailable", true, "web-speech")).toBeTruthy();
  });

  it("没听到声音和已取消不需要额外指引", () => {
    expect(voiceErrorHint("no-speech", true, "harmony")).toBeNull();
    expect(voiceErrorHint("aborted", true, "harmony")).toBeNull();
  });
});
