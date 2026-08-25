// 原生壳的推送 token 入口。
//
// 浏览器里推送走 Web Push（service worker + VAPID + pushManager），但原生壳里
// 那条路是断的：鸿蒙 ArkWeb 的 PushManager 只是 Chromium 114 留下的空壳，没接
// 投递后端（见 push-classify.ts）；国内安卓也没有 FCM。原生壳只能拿厂商推送的
// 设备 token，再由我们自己的 relay 调厂商下行接口。
//
// 分工：原生只负责「取到 token」，注册到 relay 这半边留在 web —— channel token、
// relay 连接、用户的通知开关都在这边。鸿蒙曾经在 ArkTS 里自己开 WebSocket 上报，
// 为此重写了一份 HKDF（Hkdf.ets + FleetTransport.ets，194 行），两套实现各自漂移，
// 已随本次改造删除。
//
// 壳侧约定与 shareTarget.ts 同构：`window.__fleetPushToken(token)`，早到的值先
// 堆进 `__fleetPushTokenPending`。这是所有原生壳的公共入口，不是鸿蒙专用分支 ——
// Capacitor 壳接上厂商推送后同样调它。

/** 原生壳投递设备 push token 的入口。 */
const NATIVE_PUSH_HOOK = "__fleetPushToken";
/** 壳在 hook 注册前把早到的 token 堆在这里。 */
const NATIVE_PUSH_PENDING = "__fleetPushTokenPending";

/** 最近一次收到的原生 token。push.ts 的注册/注销/重连重注都读它。 */
let nativeToken = "";

/** 当前是否跑在拿得到原生推送 token 的壳里。 */
export function hasNativePushToken(): boolean {
  return nativeToken.length > 0;
}

export function nativePushToken(): string {
  return nativeToken;
}

/**
 * 订阅原生壳投递的 push token。
 *
 * token 什么时候到是不确定的：原生要先过系统通知授权，必然晚于首帧；而本函数
 * 跑在 React effect 里，又未必早于原生。两头都不确定，所以两头都走队列 —— 注册
 * 时先把积压的消费掉。少了这步，冷启动拿到的 token 会静默丢失，表现为「装了就
 * 是收不到推送」，且没有任何报错。
 *
 * 返回退订函数。在非原生环境里这个 hook 永远不会被调用，模块整体是惰性的。
 */
export function onNativePushToken(handler: (token: string) => void): () => void {
  const deliver = (token: unknown) => {
    if (typeof token !== "string" || token.length === 0) return;
    nativeToken = token;
    handler(token);
  };

  const w = window as unknown as Record<string, unknown>;
  const pending = w[NATIVE_PUSH_PENDING];
  w[NATIVE_PUSH_HOOK] = (token: string) => deliver(token);
  if (Array.isArray(pending)) {
    for (const item of pending as unknown[]) deliver(item);
    (pending as unknown[]).length = 0;
  }

  return () => {
    delete w[NATIVE_PUSH_HOOK];
  };
}
