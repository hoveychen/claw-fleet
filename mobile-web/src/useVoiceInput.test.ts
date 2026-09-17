import { describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { voiceErrorHint, voiceErrorText } = await import("./useVoiceInput");

describe("voiceErrorText", () => {
  // The "Please enable in system settings" phrase moved to hint — leaving it in the title would
  // conflict with the "Authorize" button in the shell, and point nowhere in the browser.
  it("没权限的标题里不再自带指引", () => {
    const text = voiceErrorText("no-permission");
    expect(text).not.toContain("系统设置");
    expect(text).toBeTruthy();
  });
});

describe("voiceErrorHint", () => {
  // When we can send the user there, the text is redundant: the button itself is the guide.
  it("能拉起授权面板时不给指引文字", () => {
    expect(voiceErrorHint("no-permission", true, "harmony")).toBeNull();
  });

  // In browsers, microphone permissions live in **site settings**, not system settings. Pointing
  // to the wrong place is worse than no guidance: users will actually go through system settings,
  // come back, and find it doesn't work.
  // Assertions match both Chinese and English patterns: t() returns based on the current language,
  // so hardcoding assertions to Chinese would mean the test only works in Chinese environments.
  it("浏览器里指向站点设置而不是系统设置", () => {
    const hint = voiceErrorHint("no-permission", false, "web-speech");
    expect(hint).toMatch(/站点|site settings/i);
    expect(hint).not.toMatch(/系统设置|Settings →/);
  });

  it("壳里送不过去时才指向系统设置", () => {
    const shell = voiceErrorHint("no-permission", false, "capacitor");
    const web = voiceErrorHint("no-permission", false, "web-speech");
    expect(shell).toMatch(/系统设置|Settings/);
    // The two environments must point to **different** places; otherwise, the branching is pointless.
    expect(shell).not.toBe(web);
  });

  // These two types are unrelated to permissions, so guidance shouldn't follow the permission branching.
  it("网络与不可用各有自己的下一步", () => {
    expect(voiceErrorHint("network", true, "web-speech")).toBeTruthy();
    expect(voiceErrorHint("unavailable", true, "web-speech")).toBeTruthy();
  });

  it("没听到声音和已取消不需要额外指引", () => {
    expect(voiceErrorHint("no-speech", true, "harmony")).toBeNull();
    expect(voiceErrorHint("aborted", true, "harmony")).toBeNull();
  });
});
