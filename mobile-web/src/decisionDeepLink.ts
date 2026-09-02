// 通知点击 → 直达对应决策卡。
//
// 桌面在 notify 帧里把决策标识编进 `url` 的 fragment(`/#d=<kind>:<id>`,见
// mobile_relay 的 notify_url)。三条投递路径最终都汇到这里,web 侧因此只有一份
// 路由实现:
//
//   * PWA 冷启动 —— service worker 的 notificationclick 走 openWindow(url),
//     地址栏直接带着 fragment 进来
//   * PWA 已在前台 —— SW 只 focus 不重载,URL 一动不动,所以它额外 postMessage
//     一份 url 过来
//   * 原生壳(鸿蒙 WebShell / 后续的 Capacitor)—— 没有 service worker,由原生
//     从点击的 want 里取出 url,调 window.__fleetDeepLink 注入
//
// 与 shareTarget.ts / nativePush.ts 同构:原生 hook 带 pending 队列,因为壳完全
// 可能在这个 hook 注册之前就投递(冷启动点通知正是这种情形)。

/** 原生壳投递点击目标的入口。 */
const NATIVE_DEEPLINK_HOOK = "__fleetDeepLink";
/** 壳在 hook 注册前把早到的 url 堆在这里。 */
const NATIVE_DEEPLINK_PENDING = "__fleetDeepLinkPending";

/** 决策标识的 fragment 参数名,与 mobile_relay::notify_url 的 `/#d=` 一致。 */
const DECISION_PARAM = "d";
/** 来源 channel 的标记,由 relay 在扇出时盖上(fleet-relay/src/notify_target.rs)。 */
const CHANNEL_PARAM = "ch";

export interface DecisionTarget {
  kind: string;
  id: string;
  /** 这条通知来自哪个 channel(channel id 的前缀)。多设备之后必须有它:卡 id
   *  只在单机内唯一,两台同时有卡时,光靠 id 说不出该展开哪一张。
   *
   *  老 relay 不盖这个标记,所以是可选的 —— 缺席时调用方按 id 找第一张匹配的卡
   *  (跳错一张也好过点了没反应)。 */
  channelMark?: string;
}

/**
 * 从 notify 的 url 里解出要聚焦的决策。
 *
 * 接受完整 URL、路径,或裸 fragment —— 三条投递路径给的形状不同(地址栏是完整
 * URL,SW 转发的是 notify 原样的 `/#d=...`),统一在这里吸收差异。
 *
 * 只按**第一个**冒号切分:kind 不含冒号,而 id 是外部给的,不保证不含。
 * 拿不到 id 就返回 null —— 桌面在请求没带 id 时会把 tag 退化成裸 kind,那种
 * 链接没有可聚焦的目标,当作普通「打开应用」处理。
 */
export function parseDecisionDeepLink(url: string): DecisionTarget | null {
  const hash = url.indexOf("#");
  if (hash < 0) return null;
  // fragment 是 `&` 分隔的参数(`d=guard:g1&ch=105e300f`)。按参数解而不是
  // 「前缀 + 剩下全是 id」:relay 会在后面追加来源标记,那样解会把 `&ch=…`
  // 一起当成卡 id 的一部分。
  const params = new Map<string, string>();
  for (const part of url.slice(hash + 1).split("&")) {
    const eq = part.indexOf("=");
    if (eq <= 0) continue;
    params.set(part.slice(0, eq), part.slice(eq + 1));
  }
  const value = params.get(DECISION_PARAM);
  if (!value) return null;
  // 只按**第一个**冒号切分:kind 不含冒号,而 id 是外部给的,不保证不含。
  const colon = value.indexOf(":");
  if (colon <= 0) return null;
  const kind = value.slice(0, colon);
  const id = value.slice(colon + 1);
  if (!id) return null;
  const channelMark = params.get(CHANNEL_PARAM);
  return channelMark ? { kind, id, channelMark } : { kind, id };
}

/**
 * 订阅「点击通知要打开某张决策卡」。返回退订函数。
 *
 * 挂载时先读一次当前地址 —— 冷启动那条路径的 fragment 早就在地址栏里了,没有
 * 任何事件会补发它。
 */
export function onDecisionDeepLink(handler: (target: DecisionTarget) => void): () => void {
  const deliver = (url: unknown) => {
    if (typeof url !== "string") return;
    const target = parseDecisionDeepLink(url);
    if (target) handler(target);
  };

  // 冷启动:fragment 已经在地址栏里。
  deliver(window.location.href);

  // 同一个页面里再次点通知(浏览器会改 hash 而不重载)。
  const onHashChange = () => deliver(window.location.href);
  window.addEventListener("hashchange", onHashChange);

  // PWA 已在前台:SW 只 focus 不重载,URL 不变,所以它 postMessage 一份过来。
  const onSwMessage = (e: MessageEvent) => {
    const data = e.data as { type?: string; url?: string } | undefined;
    if (data?.type === "fleet-deeplink") deliver(data.url);
  };
  navigator.serviceWorker?.addEventListener("message", onSwMessage);

  // 原生壳注入通道。先消费积压 —— 冷启动点通知时,壳早于 React effect。
  const w = window as unknown as Record<string, unknown>;
  const pending = w[NATIVE_DEEPLINK_PENDING];
  w[NATIVE_DEEPLINK_HOOK] = (url: string) => deliver(url);
  if (Array.isArray(pending)) {
    for (const item of pending as unknown[]) deliver(item);
    (pending as unknown[]).length = 0;
  }

  return () => {
    window.removeEventListener("hashchange", onHashChange);
    navigator.serviceWorker?.removeEventListener("message", onSwMessage);
    delete w[NATIVE_DEEPLINK_HOOK];
  };
}
