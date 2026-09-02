// relay 形态的传输层工厂。**只被 main.tsx 的动态 import 引用** —— 这条间接
// 是有意的:它是 relay 客户端进入 bundle 的唯一入口,同源构建把那个 import
// 分支消掉,整棵 relay 依赖树就一起消失。直接 import 本文件会让这套安排失效。

import { getClientId } from "./clientId";
import { deviceLabel } from "./deviceLabel";
import { pushState } from "./push";
import { isMockMode } from "./mockMode";
import { MockRelayClient } from "./mock/relay";
import { HttpTransport } from "./httpTransport";
import { RelayClient, binarySupported, gzipSupported } from "./relay";
import type { PairedDevice } from "./devices";
import type { FleetTransport, TransportHandlers } from "./transport";

export function makeTransport(
  device: PairedDevice,
  handlers: TransportHandlers,
): FleetTransport {
  // `?mock` 用固定数据跑整个 UI（promo 录屏、无 relay 时改界面）。它归这里而不
  // 归 App：那个假客户端 extends RelayClient，所以它本来就只在 relay 形态下存在。
  if (isMockMode()) return new MockRelayClient(handlers);
  // 直连一台 HTTP 主机(`fleet webui` / 云容器)。这条形态下**没有**中转、没有
  // 配对密钥、也没有推送通道 —— HttpTransport 的 pushSubscribe 恒返回 false,
  // 「更多」页据此隐掉那台的推送开关。
  if (device.kind === "http") {
    return new HttpTransport(handlers, { baseUrl: device.baseUrl, token: device.token });
  }
  return new RelayClient(
    device.secret,
    handlers,
    () => {
    // 每次心跳都现读,而不是在构造时捕获 —— `pushSubscribed` 要反映当下。
    const { label, platform } = deviceLabel(navigator.userAgent);
    return {
      clientId: getClientId(),
      label,
      platform,
      pushSubscribed: pushState() === "granted",
      supportsGzip: gzipSupported(),
      supportsBinary: binarySupported(),
      // 增量应用是纯 JS(见 relay.ts 的 sessions_delta),没有需要特性检测的
      // 浏览器 API,所以恒为真。
      supportsDelta: true,
      // 每份构建固定;让桌面端能标出一个跑着旧包的设备。
      appCommit: __APP_COMMIT__,
    };
  },
    // 这台设备指名的 relay(设备簿里存的);null = 构建默认值。
    device.relayBase,
  );
}
