// Pure push-capability classification. Kept free of any module-level side
// effects (no relay/i18n imports) so it is unit-testable in a plain Node env.

export type PushState =
  | "unsupported"
  | "unsupported-harmony"
  | "ios-needs-a2hs"
  | "prompt"
  | "granted"
  | "denied";

/** Environment inputs classifyPush() decides on — plain data, no globals. */
export type PushEnv = {
  hasServiceWorker: boolean;
  hasPushManager: boolean;
  permission: NotificationPermission;
  ua: string;
  standalone: boolean;
  /** 原生壳是否已交来厂商推送的设备 token（见 nativePush.ts）。 */
  hasNativePush: boolean;
};

function isIos(ua: string): boolean {
  return /iphone|ipad|ipod/i.test(ua);
}

// HarmonyOS 5 (NEXT) 内置浏览器基于 ArkWeb 内核（按 Chromium 114 定制），UA 形如
// `... Chrome/114.0.0.0 Safari/537.36 ArkWeb/4.1.6.1 Mobile`，系统标识 `OpenHarmony`、
// 内核标识 `ArkWeb`。这两个标识精确对应「没接通 Web Push 投递后端」的引擎。
function isHarmonyArkWeb(ua: string): boolean {
  return /arkweb|openharmony/i.test(ua);
}

/** Pure classification of the push capability from the ambient environment. */
export function classifyPush(env: PushEnv): PushState {
  // 原生壳的厂商推送 token 优先于一切浏览器能力判断：它绕开 Web Push 整条链路
  // （service worker / VAPID / Notification.permission 全不参与），由 relay 直接
  // 调厂商下行接口。鸿蒙壳恰恰会同时满足下面那条 unsupported-harmony ——
  // UA 里有 ArkWeb、permission 恒为 denied —— 所以这一判断必须在最前面，否则
  // 明明能收推送的壳会被判成「不支持」，UI 连开关都不给。
  if (env.hasNativePush) {
    return "granted";
  }
  if (!env.hasServiceWorker || !env.hasPushManager) {
    return isIos(env.ua) && !env.standalone ? "ios-needs-a2hs" : "unsupported";
  }
  // 鸿蒙 ArkWeb 自带 PushManager 的「壳」（Chromium 114 遗留），但没接通 Web Push 投递
  // 后端（无 Google 服务，FCM 不通；系统推送走原生 Push Kit，不对网页开放），
  // Notification.permission 恒为 denied，且没有站点级「网页通知」开关可开。旧逻辑会把它
  // 当普通「已拒绝」，误导用户去找不存在的系统设置——识别出来当「鸿蒙不支持」处理。
  // 仅在非 granted 时降级：万一未来某版 ArkWeb 接通了 Web Push 并能授权，则正常放行。
  if (isHarmonyArkWeb(env.ua) && env.permission !== "granted") {
    return "unsupported-harmony";
  }
  switch (env.permission) {
    case "granted":
      return "granted";
    case "denied":
      return "denied";
    default:
      return "prompt";
  }
}
