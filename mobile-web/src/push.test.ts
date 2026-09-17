import { beforeEach, describe, expect, it } from "vitest";
import { classifyPush, type PushEnv } from "./push-classify";
import { isPushMuted, isPushOptedOut, setPushMuted, setPushOptedOut } from "./push";

// HarmonyOS NEXT (HarmonyOS 5) native browser / ArkWeb Web component user agent:
// Chromium 114 customized kernel with `OpenHarmony` system identifier + `ArkWeb/` kernel identifier.
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
  // Core bug: HarmonyOS native browser has Web Push infrastructure (Chromium 114 includes PushManager),
  // but the delivery backend is not wired up, so Notification.permission is always denied with no site-level notification toggle.
  // Old logic treats this as regular "denied", misleading users to find non-existent system settings. Should identify as HarmonyOS-specific unsupported.
  it("鸿蒙 ArkWeb + denied → unsupported-harmony（而非误导性的 denied）", () => {
    expect(classifyPush(base({ ua: HARMONY_UA, permission: "denied" }))).toBe("unsupported-harmony");
  });

  it("鸿蒙 ArkWeb + prompt 也 → unsupported-harmony（点开启也无效，不该给按钮）", () => {
    expect(classifyPush(base({ ua: HARMONY_UA, permission: "default" }))).toBe("unsupported-harmony");
  });

  it("鸿蒙 ArkWeb 万一能 granted → 正常放行（不误伤未来接通的可能）", () => {
    expect(classifyPush(base({ ua: HARMONY_UA, permission: "granted" }))).toBe("granted");
  });

  // Regression protection: non-HarmonyOS browser behavior remains unchanged.
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

  // Native shell (HarmonyOS WebShell / Capacitor) receives vendor push tokens
  // and bypasses Web Push entirely. HarmonyOS shell UA contains ArkWeb and permission is always denied,
  // which happens to match unsupported-harmony above. Without checking token first, shells capable of receiving
  // push would be classified as "unsupported", leaving no UI toggle to enable.
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

// After multi-device support, "mute notifications" must apply per-device only:
// home device running long tasks, office device sending cards at night—these should be handled separately.
describe("per-device mute", () => {
  beforeEach(() => localStorage.clear());

  it("mutes one device without touching the other", () => {
    setPushMuted("d1", true);
    expect(isPushMuted("d1")).toBe(true);
    expect(isPushMuted("d2")).toBe(false);
  });

  // Single-device era had only one global flag. Users who muted notifications before upgrade
  // should not suddenly start receiving them again—fall back to the global flag when no device-specific record exists.
  it("falls back to the single-device era global flag", () => {
    localStorage.setItem("fleet:push-opt-out", "1");
    expect(isPushMuted("d1")).toBe(true);
    // Once this device has its own record, it takes precedence.
    setPushMuted("d1", false);
    expect(isPushMuted("d1")).toBe(false);
    expect(isPushMuted("d2")).toBe(true);
  });

  it("the phone counts as opted out only when every device is muted", () => {
    setPushMuted("d1", true);
    expect(isPushOptedOut(["d1", "d2"])).toBe(false);
    setPushMuted("d2", true);
    expect(isPushOptedOut(["d1", "d2"])).toBe(true);
  });

  it("the master switch fans out to every device", () => {
    setPushOptedOut(true, ["d1", "d2"]);
    expect(isPushMuted("d1")).toBe(true);
    expect(isPushMuted("d2")).toBe(true);
    // Write the global flag as well—it's the default for **newly paired devices**, so a freshly added device
    // won't contradict the user's just-expressed preference and start sending notifications.
    expect(localStorage.getItem("fleet:push-opt-out")).toBe("1");
    expect(isPushMuted("d3-just-paired")).toBe(true);
  });

  it("the master switch back on unmutes everything", () => {
    setPushOptedOut(true, ["d1", "d2"]);
    setPushOptedOut(false, ["d1", "d2"]);
    expect(isPushOptedOut(["d1", "d2"])).toBe(false);
    expect(isPushMuted("d3-just-paired")).toBe(false);
  });
});
