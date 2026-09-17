// Transport factory for same-origin mode. Same signature as transportRelay.ts
// so main.tsx's choice is just swapping one import, not two separate setups.
//
// In this mode, the device book always has exactly one "same-origin" device
// (App's SAME_ORIGIN_DEVICE: kind="http", baseUrl="" = the origin that served
// this page). We still read baseUrl/token from the device record instead of
// forcing same-origin unconditionally—that way the same code can serve scenarios
// where a same-origin page points to another HTTP host, without branching again.
//
// **This file must never import relay-side modules.** This is the last gate
// ensuring same-origin builds don't include the relay client (see comments in
// hostMode.test.ts and main.tsx).

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
