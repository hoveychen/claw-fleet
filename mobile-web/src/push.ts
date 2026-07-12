// Web Push enrollment: permission → VAPID key → pushManager.subscribe →
// register the subscription on the relay channel.
//
// iOS 约束：Safari 只在「添加到主屏幕」后的 standalone PWA 里暴露 PushManager，
// 页面里要先引导用户 A2HS。

import { RelayClient, relayHttpBase } from "./relay";
import { classifyPush, type PushState } from "./push-classify";

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
  });
}

function urlBase64ToUint8Array(base64: string): Uint8Array {
  const padding = "=".repeat((4 - (base64.length % 4)) % 4);
  const normalized = (base64 + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = window.atob(normalized);
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

/** Subscribe and register on the relay. Returns the resulting PushState. */
export async function enablePush(client: RelayClient): Promise<PushState> {
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
  client.pushSubscribe(subscription.toJSON());
  return "granted";
}

/** Re-register an existing subscription after a reconnect (no prompts). */
export async function resyncPush(client: RelayClient): Promise<void> {
  if (pushState() !== "granted") return;
  try {
    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.getSubscription();
    if (subscription) client.pushSubscribe(subscription.toJSON());
  } catch {
    // best-effort
  }
}
