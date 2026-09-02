// 等一条刚建立的传输层可用。
//
// 多设备之后有一件事必须对**不是当前那一台**的设备做:移除一台时告诉它的 relay
// channel 停止推送。那台没有活着的连接,所以要临时开一条、只为发一帧、然后关掉。
// 「连上了吗」这件事在 FleetTransport 上只有 `isAuthed` 这一个诚实的答案(它对
// 同源 HTTP 恒真,对 relay 是握手完成),所以这里就轮询它 —— 事件回调那条路要
// 求调用方在构造 transport 时就把 handlers 织进来,而临时连接的调用方本来就不
// 关心那些事件。

import type { FleetTransport } from "./transport";

/** 临时连接等握手的预算。relay 握手通常在几百毫秒内完成;给到 5s 是为了覆盖
 *  弱网,再长就该如实告诉用户「没退成,下次再试」而不是继续让他等。 */
export const AUTH_WAIT_MS = 5_000;

/** 轮询间隔。够密以免在快链路上白等,又不至于把主线程占满。 */
const POLL_MS = 100;

/** 等到 `isAuthed` 为真,或超时。返回是否等到了。 */
export function waitAuthed(
  client: FleetTransport,
  budgetMs: number = AUTH_WAIT_MS,
  pollMs: number = POLL_MS,
): Promise<boolean> {
  if (client.isAuthed) return Promise.resolve(true);
  return new Promise((resolve) => {
    const deadline = Date.now() + budgetMs;
    const timer = window.setInterval(() => {
      if (client.isAuthed) {
        window.clearInterval(timer);
        resolve(true);
      } else if (Date.now() >= deadline) {
        window.clearInterval(timer);
        resolve(false);
      }
    }, pollMs);
  });
}
