import { beforeEach, describe, expect, it } from "vitest";
import { classifyPush, type PushEnv } from "./push-classify";
import { isPushOptedOut, setPushOptedOut } from "./push";

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
  hasNativePush: false,
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

  // 原生壳（鸿蒙 WebShell / Capacitor）拿到厂商推送 token 后，推送整条链路都不
  // 经过 Web Push。鸿蒙壳的 UA 里带 ArkWeb、permission 又恒为 denied，正好命中
  // 上面那条 unsupported-harmony —— 若不优先判 token，能收推送的壳会被判成
  // 「不支持」，UI 连开关都不给。
  it("原生 token 在手 → granted（哪怕 UA 是鸿蒙且 permission 为 denied）", () => {
    expect(
      classifyPush(base({ ua: HARMONY_UA, permission: "denied", hasNativePush: true })),
    ).toBe("granted");
  });

  it("原生 token 在手 → granted（哪怕连 Service Worker 都没有）", () => {
    expect(
      classifyPush(base({ hasServiceWorker: false, hasPushManager: false, hasNativePush: true })),
    ).toBe("granted");
  });
});

describe("push opt-out persistence", () => {
  beforeEach(() => localStorage.clear());

  it("默认(未设置)不算 opted out", () => {
    expect(isPushOptedOut()).toBe(false);
  });

  it("停用后 isPushOptedOut() 为真且持久化", () => {
    setPushOptedOut(true);
    expect(isPushOptedOut()).toBe(true);
    expect(localStorage.getItem("fleet:push-opt-out")).toBe("1");
  });

  it("重新开启清除 opt-out", () => {
    setPushOptedOut(true);
    setPushOptedOut(false);
    expect(isPushOptedOut()).toBe(false);
    expect(localStorage.getItem("fleet:push-opt-out")).toBe("0");
  });
});
