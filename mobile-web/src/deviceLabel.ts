// Honest UA-derived device label. The web platform does NOT expose a real
// device name (iOS/Android hide it), so this is a best-effort platform + browser
// guess — the same kind of UA sniffing push-classify.ts already relies on.
// Kept pure (no globals) so it is unit-testable in a plain Node env.

/** Machine-readable platform key — the desktop uses it to pick an icon. */
export type DevicePlatform =
  | "ios"
  | "android"
  | "harmony"
  | "windows"
  | "macos"
  | "linux"
  | "unknown";

export interface DeviceLabel {
  /** Human string like "iPhone · Safari" / "HarmonyOS · ArkWeb". */
  label: string;
  platform: DevicePlatform;
}

function detectPlatform(ua: string): { platform: DevicePlatform; name: string } {
  // 鸿蒙 ArkWeb 的 UA 同时含 Chrome/Safari 标识（Chromium 114 遗留），必须先判
  // 才不会被误当成 Android/桌面 —— 与 push-classify.ts 的 isHarmonyArkWeb 同源。
  if (/arkweb|openharmony/i.test(ua)) return { platform: "harmony", name: "HarmonyOS" };
  if (/ipad/i.test(ua)) return { platform: "ios", name: "iPad" };
  if (/iphone|ipod/i.test(ua)) return { platform: "ios", name: "iPhone" };
  // iPadOS 13+ Safari reports a desktop Mac UA; treat touch Macs as iPad.
  if (/macintosh/i.test(ua) && /mobile/i.test(ua)) return { platform: "ios", name: "iPad" };
  if (/android/i.test(ua)) return { platform: "android", name: "Android" };
  if (/windows/i.test(ua)) return { platform: "windows", name: "Windows" };
  if (/macintosh|mac os x/i.test(ua)) return { platform: "macos", name: "macOS" };
  if (/linux/i.test(ua)) return { platform: "linux", name: "Linux" };
  return { platform: "unknown", name: "未知设备" };
}

function detectBrowser(ua: string): string {
  if (/arkweb/i.test(ua)) return "ArkWeb";
  if (/edg[ei]?\//i.test(ua)) return "Edge";
  if (/(opr|opera)\//i.test(ua)) return "Opera";
  if (/firefox|fxios/i.test(ua)) return "Firefox";
  // CriOS = Chrome on iOS; plain Chrome must come after the Edge/Opera checks
  // above since those UAs also contain "Chrome".
  if (/crios|chrome|chromium/i.test(ua)) return "Chrome";
  if (/safari/i.test(ua)) return "Safari";
  return "";
}

/** Classify a user-agent string into a display label + platform key. */
export function deviceLabel(ua: string): DeviceLabel {
  const { platform, name } = detectPlatform(ua);
  const browser = detectBrowser(ua);
  return { platform, label: browser ? `${name} · ${browser}` : name };
}
