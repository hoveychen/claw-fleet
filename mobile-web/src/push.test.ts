import { describe, expect, it } from "vitest";
import { classifyPush, type PushEnv } from "./push-classify";

// HarmonyOS NEXT (鸿蒙5) 内置浏览器 / ArkWeb Web 组件的真实 UA 形态：
// Chromium 114 定制内核，含 `OpenHarmony` 系统标识 + `ArkWeb/` 内核标识。
const HARMONY_UA =
  "Mozilla/5.0 (Phone; OpenHarmony 5.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36 ArkWeb/4.1.6.1 Mobile";
const CHROME_ANDROID_UA =
  "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36";
const SAFARI_IOS_UA =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

const base = (over: Partial<PushEnv>): PushEnv => ({
  hasServiceWorker: true,
  hasPushManager: true,
  permission: "default",
  ua: CHROME_ANDROID_UA,
  standalone: false,
  ...over,
});

describe("classifyPush", () => {
  // 核心 bug：鸿蒙内置浏览器有 Web Push 的「壳」(Chromium 114 自带 PushManager)，
  // 但没接通投递后端，Notification.permission 恒为 denied，且没有站点级通知开关可开。
  // 旧逻辑会把它当成普通「已拒绝」，误导用户去找不存在的系统设置。应识别为鸿蒙专属不支持。
  it("鸿蒙 ArkWeb + denied → unsupported-harmony（而非误导性的 denied）", () => {
    expect(classifyPush(base({ ua: HARMONY_UA, permission: "denied" }))).toBe("unsupported-harmony");
  });

  it("鸿蒙 ArkWeb + prompt 也 → unsupported-harmony（点开启也无效，不该给按钮）", () => {
    expect(classifyPush(base({ ua: HARMONY_UA, permission: "default" }))).toBe("unsupported-harmony");
  });

  it("鸿蒙 ArkWeb 万一能 granted → 正常放行（不误伤未来接通的可能）", () => {
    expect(classifyPush(base({ ua: HARMONY_UA, permission: "granted" }))).toBe("granted");
  });

  // 回归保护：非鸿蒙浏览器行为完全不变。
  it("普通 Chrome + denied → denied（真实用户拒绝，仍提示去设置开启）", () => {
    expect(classifyPush(base({ permission: "denied" }))).toBe("denied");
  });

  it("普通 Chrome + granted → granted", () => {
    expect(classifyPush(base({ permission: "granted" }))).toBe("granted");
  });

  it("普通 Chrome + default → prompt", () => {
    expect(classifyPush(base({ permission: "default" }))).toBe("prompt");
  });

  it("无 PushManager 的 iOS 非 standalone → ios-needs-a2hs", () => {
    expect(
      classifyPush(base({ hasPushManager: false, ua: SAFARI_IOS_UA, standalone: false })),
    ).toBe("ios-needs-a2hs");
  });

  it("无 Service Worker 的桌面浏览器 → unsupported", () => {
    expect(classifyPush(base({ hasServiceWorker: false }))).toBe("unsupported");
  });
});
