// Web Push enrollment: permission → VAPID key → pushManager.subscribe →
// register the subscription on the relay channel.
//
// iOS 约束：Safari 只在「添加到主屏幕」后的 standalone PWA 里暴露 PushManager，
// 页面里要先引导用户 A2HS。

import { relayHttpBase } from "./relay";
import type { FleetTransport } from "./transport";
import { classifyPush, type PushState } from "./push-classify";
import { hasNativePushToken, nativePushToken } from "./nativePush";

export type { PushState } from "./push-classify";

function isStandalone(): boolean {
  return (
    window.matchMedia("(display-mode: standalone)").matches ||
    (navigator as unknown as { standalone?: boolean }).standalone === true
  );
}

export function pushState(): PushState {
  return classifyPush({
    hasServiceWorker: "serviceWorker" in navigator,
    hasPushManager: "PushManager" in window,
    permission: typeof Notification !== "undefined" ? Notification.permission : "denied",
    ua: navigator.userAgent,
    standalone: isStandalone(),
    hasNativePush: hasNativePushToken(),
  });
}

// A user who explicitly turns notifications OFF still has a "granted" browser
// permission (the browser gives no API to revoke it — only the OS settings do),
// so `pushState()` alone can't tell "on" from "off". We persist the opt-out so
// the auto-(re)subscribe paths (mount effect + reconnect resync) don't
// resurrect a subscription the user just removed. Follows theme.ts / wakeLock.ts
// localStorage "1"/"0" convention.
const PUSH_OPT_OUT_KEY = "fleet:push-opt-out";

export function isPushOptedOut(): boolean {
  if (typeof localStorage === "undefined") return false;
  return localStorage.getItem(PUSH_OPT_OUT_KEY) === "1";
}

export function setPushOptedOut(optedOut: boolean): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(PUSH_OPT_OUT_KEY, optedOut ? "1" : "0");
}

function urlBase64ToUint8Array(base64: string): Uint8Array {
  const padding = "=".repeat((4 - (base64.length % 4)) % 4);
  const normalized = (base64 + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = window.atob(normalized);
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

/** Subscribe and register on the relay. Returns the resulting PushState. */
export async function enablePush(client: FleetTransport): Promise<PushState> {
  // 原生壳：token 已由壳交来（系统通知授权也在壳里问过了），这里只剩注册。
  // 不走 Notification.requestPermission —— WebView 里那个 API 要么不存在、要么
  // 恒返回 denied，调了只会把已经能用的推送判死。
  if (hasNativePushToken()) {
    client.pushSubscribe({ platform: "harmony", token: nativePushToken() });
    setPushOptedOut(false);
    return "granted";
  }
  const state = pushState();
  if (
    state === "unsupported" ||
    state === "unsupported-harmony" ||
    state === "ios-needs-a2hs" ||
    state === "denied"
  ) {
    return state;
  }
  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    return permission === "denied" ? "denied" : "prompt";
  }
  const registration = await navigator.serviceWorker.ready;
  const res = await fetch(`${relayHttpBase().replace(/\/$/, "")}/vapid`);
  const { publicKey } = (await res.json()) as { publicKey: string };
  const subscription =
    (await registration.pushManager.getSubscription()) ??
    (await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: urlBase64ToUint8Array(publicKey) as BufferSource,
    }));
  // Tag the platform so the relay can route Web Push vs HarmonyOS Push Kit
  // subscriptions on the same channel (relay defaults absent platform to web).
  client.pushSubscribe({ ...subscription.toJSON(), platform: "web" });
  // Enabling clears any prior explicit opt-out.
  setPushOptedOut(false);
  return "granted";
}

/** Turn notifications off: tell the relay to drop the subscription, unsubscribe
 *  in the browser, and persist the opt-out so nothing re-subscribes. Sends the
 *  unsubscribe frame BEFORE `subscription.unsubscribe()` so the endpoint the
 *  relay keys on is still available. Best-effort — always records the opt-out. */
export async function disablePush(client: FleetTransport): Promise<void> {
  setPushOptedOut(true);
  if (hasNativePushToken()) {
    // 原生 token 由系统签发，web 侧撤不掉，只能让 relay 别再往它发。
    client.pushUnsubscribe({ platform: "harmony", token: nativePushToken() });
    return;
  }
  try {
    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.getSubscription();
    if (subscription) {
      client.pushUnsubscribe({ endpoint: subscription.endpoint, platform: "web" });
      await subscription.unsubscribe();
    }
  } catch {
    // best-effort
  }
}
