// Transport layer factory for relay mode. **Referenced only via dynamic import from
// main.tsx** — this indirection is intentional: it's the only entry point for relay
// client into the bundle, and same-origin builds can eliminate that import branch,
// making the entire relay dependency tree vanish. Direct imports break this.

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
  // `?mock` runs the whole UI with fixed data (promo screenshots, testing without
  // relay). It belongs here not in App: that mock client extends RelayClient, so it
  // only exists in relay mode anyway.
  if (isMockMode()) return new MockRelayClient(handlers);
  // Direct HTTP host connection (`fleet webui` / cloud container). In this mode there
  // is **no** relay, no paired keys, no push channel — HttpTransport's pushSubscribe
  // always returns false, and the "More" page hides that device's push toggle based on
  // that.
  if (device.kind === "http") {
    return new HttpTransport(handlers, { baseUrl: device.baseUrl, token: device.token });
  }
  return new RelayClient(
    device.secret,
    handlers,
    () => {
      // Read on every heartbeat, not captured at construction — `pushSubscribed` must
      // reflect the current state.
      const { label, platform } = deviceLabel(navigator.userAgent);
      return {
        clientId: getClientId(),
        label,
        platform,
        pushSubscribed: pushState() === "granted",
        supportsGzip: gzipSupported(),
        supportsBinary: binarySupported(),
        // Incremental apply is pure JS (see sessions_delta in relay.ts), has no browser
        // APIs to feature-detect, so always true.
        supportsDelta: true,
        // Fixed per build; lets desktop identify a device running an old package.
        appCommit: __APP_COMMIT__,
      };
    },
    // This device's designated relay (stored in device book); null = build default.
    device.relayBase,
  );
}
