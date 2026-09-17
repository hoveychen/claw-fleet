// Who hosts this bundle — compile-time constant, not runtime detection.
//
// Mobile UI has two deployment modes; they differ not in styling but in "who's the backend":
//
//   - **relay mode** (default): PWA on phone, Harmony shell, Capacitor package. Desktop is
//     not on the same network, so: go through relay, need pairing key, use WebSocket,
//     push via VAPID subscriptions on relay.
//   - **webui mode**: `fleet webui` serves this page and its data routes from one port.
//     Backend is at `window.location.origin`, no relay, no pairing, no relay client.
//
// Why **compile-time** constant, not runtime check: webui mode's hard requirement is that
// the bundle contains zero relay client. `relay.ts` executes `resolveRelayBase()` at module
// load time to resolve a relay address; hitting that import chain defeats tree-shaking.
// Only constants let Rollup drop the other branch and its dynamic imports; `if (runtimeCheck)`
// gets both sides bundled.
//
// Value injected by vite.config.ts via `--mode` (VITE_FLEET_HOST). Default is relay,
// so existing builds (relay mirror / Harmony sync-web / Capacitor) need no changes.

const HOST = import.meta.env.VITE_FLEET_HOST ?? "relay";

/** Same-origin deployment: data routes and page from same origin, both from `fleet webui`. */
export const IS_WEBUI = HOST === "webui";

/** Whether pairing keys are needed to start. In same-origin, the backend is the process
 *  that served the page, no third party to pair with, so no gate — access control is
 *  handled by the gateway in front of webui. */
export const NEEDS_PAIRING = !IS_WEBUI;

/** Whether push channels exist. Web Push VAPID subscriptions are registered on relay;
 *  same-origin deliberately doesn't touch relay, so no push. UI hides the push switch
 *  rather than showing a toggle that does nothing. */
export const SUPPORTS_PUSH = !IS_WEBUI;
