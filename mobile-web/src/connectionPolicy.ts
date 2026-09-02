// 多设备的连接策略:什么时候连、隔多久问一次。
//
// 单设备时这些常数怎么定都无所谓,N 台之后它们要乘以 N:三台设备就是三条心跳、
// 三条今日花费轮询、三条对账轮询。在弱网或省电场景下,那不是「稍慢一点」,而是
// 把本来就窄的链路占满 —— 而其中大部分请求问的是用户此刻根本没在看的设备。
//
// 策略拆成纯函数放在这里(而不是散在 effect 里的几个 if),因为它是一组有内容的
// 取舍,值得被单测钉住:
//
//   * **后台不连。**页面隐藏够久就把 socket 全部关掉。后台通道本来就是推送
//     (订阅登记在 relay 上,与 socket 在不在无关),继续挂着 N 条连接只是在烧电。
//     给一个宽限期,免得切个应用回来就要重握手 N 次。
//   * **非当前设备问得更慢。**今日花费是头部那个数字,合并后要问每一台;但用户
//     正在看的那台变化最相关,其余的慢一档足够。
//   * **错峰。**N 条连接同时重连会在网络恢复的那一刻挤成一堆;按设备序号错开
//     几百毫秒,代价是感知不到的延迟。

/** 页面隐藏多久之后断开所有连接。
 *
 *  短了会让「切出去回个消息再回来」每次都付一次重握手;长了则在真正切走之后还
 *  白挂着 N 条 socket。30 秒够覆盖大多数「瞄一眼就回来」的场景。 */
export const HIDDEN_DISCONNECT_MS = 30_000;

/** 当前作用域设备的今日花费轮询间隔。 */
export const USAGE_POLL_ACTIVE_MS = 20_000;
/** 其余设备的。它们的数字只进头部那个求和,慢一档没人看得出来。 */
export const USAGE_POLL_BACKGROUND_MS = 60_000;

/** 每台设备连接前的错峰延迟。 */
export const CONNECT_STAGGER_MS = 250;
/** 错峰的上限:设备再多也不该让最后一台等太久。 */
export const CONNECT_STAGGER_MAX_MS = 1_500;

export interface VisibilityState {
  visible: boolean;
  /** 进入隐藏的时刻;`visible` 为真时无意义。 */
  hiddenSince: number;
}

/** 此刻该不该保持这条连接。
 *
 *  隐藏**未满**宽限期时仍然保持 —— 那通常只是切出去看一眼。 */
export function shouldConnect(vis: VisibilityState, now: number): boolean {
  if (vis.visible) return true;
  return now - vis.hiddenSince < HIDDEN_DISCONNECT_MS;
}

/** 这台设备的今日花费轮询间隔。 */
export function usagePollMs(isActive: boolean): number {
  return isActive ? USAGE_POLL_ACTIVE_MS : USAGE_POLL_BACKGROUND_MS;
}

/** 第 `index` 台设备的错峰延迟。 */
export function connectDelayMs(index: number): number {
  return Math.min(index * CONNECT_STAGGER_MS, CONNECT_STAGGER_MAX_MS);
}
