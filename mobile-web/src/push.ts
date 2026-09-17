// Web Push enrollment: permission → VAPID key → pushManager.subscribe →
// register the subscription on the relay channel.
//
// iOS constraint: Safari only exposes PushManager in standalone PWA mode after
// "Add to Home Screen". Page must guide users through A2HS first.

import { SUPPORTS_PUSH } from "./hostMode";
import { relayBaseFor } from "./relayBase";
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
  // Same-origin deployment has no push channel at all (VAPID subscription is
  // registered on the relay, which this deployment is designed not to touch).
  // Return early to avoid optimistic "browser supports it" conclusion from the
  // feature-detection chain below — the constraint is deployment capacity, not
  // browser capability.
  if (!SUPPORTS_PUSH) return "unsupported";
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

/** Mute bit per device. With multiple devices, "turn off notifications" must
 *  affect only one device — e.g., the home device running a long task and the
 *  office device receiving cards at midnight should be controlled separately. */
function mutedKey(deviceId: string): string {
  return `${PUSH_OPT_OUT_KEY}:${deviceId}`;
}

/** Whether notifications are muted on this device.
 *
 *  In single-device era, there was only one global bit, so when a device has
 *  no own record, fall back to it — users who disabled notifications before
 *  upgrade should not suddenly start receiving them after. */
export function isPushMuted(deviceId: string): boolean {
  if (typeof localStorage === "undefined") return false;
  const own = localStorage.getItem(mutedKey(deviceId));
  if (own !== null) return own === "1";
  return localStorage.getItem(PUSH_OPT_OUT_KEY) === "1";
}

export function setPushMuted(deviceId: string, muted: boolean): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(mutedKey(deviceId), muted ? "1" : "0");
}

/** Whether this phone's notifications are globally muted — only true if **every**
 *  device is muted. The header banner "enable notifications" and the global
 *  toggle on the More page check this value. */
export function isPushOptedOut(deviceIds: string[] = []): boolean {
  if (typeof localStorage === "undefined") return false;
  if (deviceIds.length === 0) return localStorage.getItem(PUSH_OPT_OUT_KEY) === "1";
  return deviceIds.every((id) => isPushMuted(id));
}

/** Master switch: changes all devices at once and also sets the global bit —
 *  the latter is the default for **newly paired devices** (a just-added device
 *  has no own record yet, so falling back to the global bit honors user intent). */
export function setPushOptedOut(optedOut: boolean, deviceIds: string[] = []): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(PUSH_OPT_OUT_KEY, optedOut ? "1" : "0");
  for (const id of deviceIds) setPushMuted(id, optedOut);
}

function urlBase64ToUint8Array(base64: string): Uint8Array {
  const padding = "=".repeat((4 - (base64.length % 4)) % 4);
  const normalized = (base64 + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = window.atob(normalized);
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

/** Subscribe and register on the relay. Returns the resulting PushState. */
export async function enablePush(
  client: FleetTransport,
  relayBase?: string | null,
  /** Which device's channel to register the subscription on. If provided, also
   *  clears that device's mute bit — a device the user just enabled should not
   *  still be silenced by a prior opt-out. */
  deviceId?: string,
): Promise<PushState> {
  // Native shell: token is already provided by the shell (system notification
  // permission was already requested there), so only registration remains.
  // Skip Notification.requestPermission — that API either doesn't exist in
  // WebView or always returns denied; calling it would incorrectly mark
  // functional push as denied.
  if (hasNativePushToken()) {
    client.pushSubscribe({ platform: "harmony", token: nativePushToken() });
    if (deviceId) setPushMuted(deviceId, false);
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
  // VAPID public key is **per relay** (subscriptions are bound to the key), so
  // which key to fetch is determined by the device's relay provided by caller —
  // using the wrong key with devices on different relays silently breaks
  // delivery. Address calculation goes through relayBase.ts leaf module, so
  // same-origin builds no longer need to dynamically import the entire relay
  // client just to compute an address.
  const res = await fetch(`${relayBaseFor(relayBase).replace(/\/$/, "")}/vapid`);
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
  // Enabling clears any prior explicit opt-out — for this device only.
  if (deviceId) setPushMuted(deviceId, false);
  return "granted";
}

/** Unsubscribe just this one channel: tell the device's relay to stop sending
 *  to this subscription, but do NOT touch the browser subscription itself or
 *  the global mute toggle.
 *
 *  Use this when removing a single device. The key difference is that the
 *  browser subscription is **shared by all devices** (one endpoint is registered
 *  on N channels; see fleet-relay's push.rs: one subscription file per channel).
 *  So `subscription.unsubscribe()` here would be wrong — it would silence all
 *  other devices' notifications too.
 *
 *  Returns whether the unsubscribe frame was sent. false means "relay may still
 *  have this subscription", so caller should either retry or inform the user. */
export async function unsubscribeChannel(client: FleetTransport): Promise<boolean> {
  if (hasNativePushToken()) {
    return client.pushUnsubscribe({ platform: "harmony", token: nativePushToken() });
  }
  try {
    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.getSubscription();
    if (!subscription) return true; // never subscribed, nothing to unsubscribe from
    return client.pushUnsubscribe({ endpoint: subscription.endpoint, platform: "web" });
  } catch {
    return false;
  }
}

/** Turn notifications off: tell the relay to drop the subscription, unsubscribe
 *  in the browser, and persist the opt-out so nothing re-subscribes. Sends the
 *  unsubscribe frame BEFORE `subscription.unsubscribe()` so the endpoint the
 *  relay keys on is still available. Best-effort — always records the opt-out. */
export async function disablePush(
  client: FleetTransport,
  deviceId?: string,
): Promise<void> {
  if (deviceId) setPushMuted(deviceId, true);
  if (hasNativePushToken()) {
    // Native token is issued by the system and can't be revoked from web side,
    // so just tell relay to stop sending to it.
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
