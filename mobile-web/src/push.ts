// Web Push enrollment: permission → VAPID key → pushManager.subscribe →
// register the subscription on the relay channel.
//
// iOS 约束：Safari 只在「添加到主屏幕」后的 standalone PWA 里暴露 PushManager，
// 页面里要先引导用户 A2HS。

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
  // 同源形态压根没有推送通道（VAPID 订阅登记在 relay 上，而这条部署按设计不碰
  // relay）。先答再说，免得下面那串浏览器特性检测报出一个「浏览器支持」的乐观
  // 结论 —— 缺的不是浏览器能力，是这条部署的能力。
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

/** 每台设备各自的静音位。多设备之后「关掉通知」必须能只关一台 —— 家里那台在
 *  跑长任务、公司那台在半夜发卡,这两件事应该能分开处置。 */
function mutedKey(deviceId: string): string {
  return `${PUSH_OPT_OUT_KEY}:${deviceId}`;
}

/** 这台设备的通知是否被用户关掉。
 *
 *  单设备时代只有一个全局位,所以没有本设备记录时回落到它 —— 升级前把通知关掉
 *  的用户,升级后不该突然又开始收到推送。 */
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

/** 这部手机整体是否静音 —— 只有**每一台**都被关掉才算。头部那条「开启通知」的
 *  横幅与「更多」页那个总开关看的是它。 */
export function isPushOptedOut(deviceIds: string[] = []): boolean {
  if (typeof localStorage === "undefined") return false;
  if (deviceIds.length === 0) return localStorage.getItem(PUSH_OPT_OUT_KEY) === "1";
  return deviceIds.every((id) => isPushMuted(id));
}

/** 总开关:一次改掉全部设备,并把全局位也写上 —— 后者是**新配对设备**的默认值
 *  (刚扫进来的那台还没有自己的记录,回落到全局位才不会违背用户刚表达的意愿)。 */
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
  /** 这条订阅登记在哪台设备的 channel 上。给了就顺手把那台的静音位清掉 ——
   *  用户刚亲手开的这台,不该还被上一次的静音压着。 */
  deviceId?: string,
): Promise<PushState> {
  // 原生壳：token 已由壳交来（系统通知授权也在壳里问过了），这里只剩注册。
  // 不走 Notification.requestPermission —— WebView 里那个 API 要么不存在、要么
  // 恒返回 denied，调了只会把已经能用的推送判死。
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
  // VAPID 公钥是**每个 relay 各自**的一把（订阅绑在公钥上），所以取哪一把由
  // 调用方给的设备 relay 决定 —— 两台设备挂在不同 relay 上时，用错一把会让订阅
  // 静默收不到通知。地址计算走 relayBase.ts 这个叶子模块，同源构建因此不再需要
  // 为了一个地址动态 import 整个 relay 客户端。
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
  // Enabling clears any prior explicit opt-out — 只清这一台的。
  if (deviceId) setPushMuted(deviceId, false);
  return "granted";
}

/** 只让**这一个 channel** 停止推送:告诉这台设备的 relay 别再往这个订阅发,
 *  但**不动**浏览器订阅本体,也不碰全局静音开关。
 *
 *  移除某一台设备时用它。这里的关键区别是浏览器订阅是**所有设备共用的一份**
 *  (一个 endpoint 注册在 N 个 channel 下,见 fleet-relay 的 push.rs:每个
 *  channel 一个订阅文件)。所以 `subscription.unsubscribe()` 在这里是错的 ——
 *  那会把其他设备的通知一起掐掉。
 *
 *  返回是否把退订帧发出去了。false 意味着「relay 那边可能还留着这个订阅」,
 *  调用方要么重试、要么如实告诉用户。 */
export async function unsubscribeChannel(client: FleetTransport): Promise<boolean> {
  if (hasNativePushToken()) {
    return client.pushUnsubscribe({ platform: "harmony", token: nativePushToken() });
  }
  try {
    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.getSubscription();
    if (!subscription) return true; // 本来就没订阅,无事可退
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
