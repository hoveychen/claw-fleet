// 同源形态的传输层工厂。与 transportRelay.ts 同签名,好让 main.tsx 那处选择
// 只是换一个 import 说明符,而不是两套装配代码。
//
// `secret` 收下但不用:同源下没有配对这回事,签名一致是为了让调用点不必分叉。

import { HttpTransport } from "./httpTransport";
import type { FleetTransport, TransportHandlers } from "./transport";

export function makeTransport(
  _secret: string,
  handlers: TransportHandlers,
): FleetTransport {
  return new HttpTransport(handlers);
}
