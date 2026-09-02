// 同源形态的传输层工厂。与 transportRelay.ts 同签名,好让 main.tsx 那处选择只是
// 换一个 import 说明符,而不是两套装配代码。
//
// 这一形态下设备簿里永远只有一台「同源」设备(App 的 SAME_ORIGIN_DEVICE:
// kind 为 http、baseUrl 为空 = 就问发出这张页面的那个 origin)。这里仍然按设备
// 记录取 baseUrl/token,而不是无条件走同源 —— 那样同一份代码也能服务「同源页面
// 指向另一台 HTTP 主机」的情形,不必为它再分一次叉。
//
// **本文件不得 import relay 侧模块**,这是同源产物不含 relay 客户端的最后一道
// 关口(见 hostMode.test.ts 与 main.tsx 的注释)。

import { HttpTransport } from "./httpTransport";
import type { PairedDevice } from "./devices";
import type { FleetTransport, TransportHandlers } from "./transport";

export function makeTransport(
  device: PairedDevice,
  handlers: TransportHandlers,
): FleetTransport {
  const http = device.kind === "http" ? device : null;
  return new HttpTransport(handlers, {
    baseUrl: http?.baseUrl ?? "",
    token: http?.token ?? null,
  });
}
