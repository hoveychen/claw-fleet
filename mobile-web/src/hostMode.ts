// 这份 bundle 被谁托管 —— 编译期常量,不是运行时探测。
//
// 移动端 UI 有两种部署形态,它们的差别不在样式而在「后端是谁」:
//
//   - **relay 形态**(默认):PWA 装在手机上、鸿蒙壳、Capacitor 包。桌面端不在
//     同一张网里,所以要经中转、要配对密钥、要 WebSocket,推送靠 relay 上的
//     VAPID 订阅。
//   - **webui 形态**:`fleet webui` 从同一个端口发出这张页面和它的数据路由。
//     后端就在 `window.location.origin` 上,没有中转、没有配对、没有 relay。
//
// 为什么是**编译期**常量而不是运行时判断:webui 形态的硬要求是产物里根本不
// 含 relay 客户端。`relay.ts` 在模块加载时就会执行 `resolveRelayBase()` 去解
// 析一个 relay 地址,一条 import 链碰到它就前功尽弃。只有常量才能让 Rollup
// 把另一条分支连同它的动态 import 一起消掉;`if (runtimeCheck)` 两边都会被
// 打进去。
//
// 值由 vite.config.ts 按 `--mode` 注入(`VITE_FLEET_HOST`)。缺省是 relay,
// 所以既有的三条构建路径(relay 镜像 / 鸿蒙 sync-web / Capacitor)什么都不用改。

const HOST = import.meta.env.VITE_FLEET_HOST ?? "relay";

/** 同源部署:数据路由与这张页面同源,`fleet webui` 一并发出。 */
export const IS_WEBUI = HOST === "webui";

/** 是否需要配对密钥才能开始。同源下后端就是发页面的那个进程,没有需要配对的
 *  第三方,所以这道门整个不存在 —— 访问控制由 webui 前面的网关负责。 */
export const NEEDS_PAIRING = !IS_WEBUI;

/** 是否有推送通道。Web Push 的 VAPID 订阅登记在 relay 上,同源形态按设计不碰
 *  relay,所以这里没有推送。UI 据此隐掉推送开关,而不是摆一个开了也不响的。 */
export const SUPPORTS_PUSH = !IS_WEBUI;
