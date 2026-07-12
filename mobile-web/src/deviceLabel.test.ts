import { describe, expect, it } from "vitest";
import { deviceLabel } from "./deviceLabel";

const HARMONY_UA =
  "Mozilla/5.0 (Phone; OpenHarmony 5.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36 ArkWeb/4.1.6.1 Mobile";
const CHROME_ANDROID_UA =
  "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36";
const SAFARI_IOS_UA =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
const EDGE_WIN_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0";
const SAFARI_MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

describe("deviceLabel", () => {
  it("classifies HarmonyOS ArkWeb before Android/desktop despite Chrome+Safari tokens", () => {
    expect(deviceLabel(HARMONY_UA)).toEqual({ platform: "harmony", label: "HarmonyOS · ArkWeb" });
  });

  it("classifies Android Chrome", () => {
    expect(deviceLabel(CHROME_ANDROID_UA)).toEqual({ platform: "android", label: "Android · Chrome" });
  });

  it("classifies iOS Safari", () => {
    expect(deviceLabel(SAFARI_IOS_UA)).toEqual({ platform: "ios", label: "iPhone · Safari" });
  });

  it("classifies Edge on Windows (not Chrome)", () => {
    expect(deviceLabel(EDGE_WIN_UA)).toEqual({ platform: "windows", label: "Windows · Edge" });
  });

  it("classifies desktop Safari on macOS", () => {
    expect(deviceLabel(SAFARI_MAC_UA)).toEqual({ platform: "macos", label: "macOS · Safari" });
  });

  it("falls back for an unrecognized UA", () => {
    expect(deviceLabel("weird-bot/1.0")).toEqual({ platform: "unknown", label: "未知设备" });
  });
});
