// Entry point for native shell push tokens.
//
// Push on the browser uses Web Push (service worker + VAPID + pushManager), but the native shell
// cannot use that path: HarmonyOS ArkWeb's PushManager is just an empty shell left by Chromium 114,
// with no backend integration (see push-classify.ts); domestic Android also lacks FCM. The native shell
// can only obtain device tokens from vendors and use our own relay to call the vendor's downstream interface.
//
// Division of labor: the native side only handles "obtaining the token"; registration to the relay
// and the rest is handled on the web — channel token, relay connection, and user notification settings
// are all here. HarmonyOS previously opened its own WebSocket in ArkTS for reporting, rewriting an
// HKDF implementation (Hkdf.ets + FleetTransport.ets, 194 lines); the two implementations drifted
// apart and were removed in this refactor.
//
// Shell-side contract, isomorphic to shareTarget.ts: `window.__fleetPushToken(token)`. Early-arriving
// values are queued in `__fleetPushTokenPending`. This is the common entry point for all native shells,
// not a HarmonyOS-specific branch — the Capacitor shell also calls it after wiring up vendor push.

/** Entry point for native shells to deliver device push tokens. */
const NATIVE_PUSH_HOOK = "__fleetPushToken";
/** Queue where the shell buffers early-arriving tokens before the hook is registered. */
const NATIVE_PUSH_PENDING = "__fleetPushTokenPending";

/** Most recently received native token. Used by push.ts for registration/unregistration/reconnection. */
let nativeToken = "";

/** Whether the current shell supports obtaining native push tokens. */
export function hasNativePushToken(): boolean {
  return nativeToken.length > 0;
}

export function nativePushToken(): string {
  return nativeToken;
}

/**
 * Subscribe to native shell push tokens.
 *
 * The arrival time of tokens is uncertain: the native side must first obtain system notification
 * permissions, which necessarily comes after the first frame; this function runs in a React effect,
 * which may or may not come before the token. Since timing is uncertain on both sides, both use a queue —
 * on registration, we consume any buffered tokens first. Without this step, tokens received on cold start
 * would be silently lost, appearing as "push notifications never arrive even after installation" with no error.
 *
 * Returns an unsubscribe function. In non-native environments, this hook is never called; the module
 * is lazy overall.
 */
export function onNativePushToken(handler: (token: string) => void): () => void {
  const deliver = (token: unknown) => {
    if (typeof token !== "string" || token.length === 0) return;
    nativeToken = token;
    handler(token);
  };

  const w = window as unknown as Record<string, unknown>;
  const pending = w[NATIVE_PUSH_PENDING];
  w[NATIVE_PUSH_HOOK] = (token: string) => deliver(token);
  if (Array.isArray(pending)) {
    for (const item of pending as unknown[]) deliver(item);
    (pending as unknown[]).length = 0;
  }

  return () => {
    delete w[NATIVE_PUSH_HOOK];
  };
}
