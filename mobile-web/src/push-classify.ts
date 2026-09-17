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
  /** Whether the native shell has already provided the vendor push device token (see nativePush.ts). */
  hasNativePush: boolean;
};

function isIos(ua: string): boolean {
  return /iphone|ipad|ipod/i.test(ua);
}

// HarmonyOS 5 (NEXT)'s built-in browser uses the ArkWeb engine (customized per Chromium 114), with UA like
// `... Chrome/114.0.0.0 Safari/537.36 ArkWeb/4.1.6.1 Mobile`. System ID `OpenHarmony`,
// engine ID `ArkWeb`. These two identifiers correspond precisely to the engine with no Web Push delivery backend.
function isHarmonyArkWeb(ua: string): boolean {
  return /arkweb|openharmony/i.test(ua);
}

/** Pure classification of the push capability from the ambient environment. */
export function classifyPush(env: PushEnv): PushState {
  // The native shell's vendor push token takes priority over any browser capability check:
  // it bypasses the entire Web Push pipeline (service worker / VAPID / Notification.permission
  // all uninvolved), and the relay calls the vendor downlink API directly. Harmony shell
  // will match the unsupported-harmony condition below at the same time — UA has ArkWeb,
  // permission is always denied — so this check must come first, or a shell that can receive
  // pushes will be mistaken for unsupported, and the UI won't even show the toggle.
  if (env.hasNativePush) {
    return "granted";
  }
  if (!env.hasServiceWorker || !env.hasPushManager) {
    return isIos(env.ua) && !env.standalone ? "ios-needs-a2hs" : "unsupported";
  }
  // Harmony ArkWeb comes with a PushManager "shell" (Chromium 114 legacy) but has not connected
  // to the Web Push delivery backend (no Google services, FCM blocked; system push uses native Push Kit,
  // not exposed to web). Notification.permission is always denied, and there's no site-level
  // "web notification" toggle to enable. Old logic would treat it as a normal "denied" state,
  // misleading users to find non-existent system settings — recognize it and treat it as
  // "Harmony unsupported". Only downgrade when not granted: if some future ArkWeb version
  // connects Web Push and allows authorization, let it through normally.
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
