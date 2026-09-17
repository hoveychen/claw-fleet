// Native keep-screen-on fallback — Capacitor shell only.
//
// Why needed: iOS WKWebView only got Screen Wake Lock at **18.4**; versions
// 16.4–18.3 don't work in standalone Web App (WebKit bug 254545). So the
// standard path in wakeLock.ts is a no-op on a good chunk of deployed iPhones
// — and voice input is exactly the "mid-interaction screen off = wasted
// recording" scenario, so no-op means unfixed.
//
// @capacitor-community/keep-awake wraps iOS `UIApplication.isIdleTimerDisabled`
// and Android `FLAG_KEEP_SCREEN_ON`. We pick it over writing our own plugin
// for the same reason as voiceCapacitor.ts choosing @capgo: rolling our own
// means maintaining Swift + Kotlin.
//
// It's installed only **in shell, when standard API is absent**: Android
// WebView (84+) and iOS 18.4+ have native navigator.wakeLock; HarmonyOS
// WebShell injects its own polyfill. Those all use the standard path.

import { Capacitor } from "@capacitor/core";
import { KeepAwake } from "@capacitor-community/keep-awake";
import { setWakeLockFallback, type WakeLockLike, type WakeLockSentinelLike } from "./wakeLock";

/** Wrap keep-awake into the shape wakeLock.ts understands. */
export function nativeWakeLock(): WakeLockLike {
  return {
    async request(): Promise<WakeLockSentinelLike> {
      await KeepAwake.keepAwake();
      const sentinel: WakeLockSentinelLike = {
        released: false,
        async release() {
          sentinel.released = true;
          await KeepAwake.allowSleep();
        },
        // Native path has no "system takes it away" event (background switch is
        // handled by wakeLock.ts's visibilitychange), so nothing to forward.
        addEventListener() {},
      };
      return sentinel;
    },
  };
}

/**
 * Call once at startup. Silent no-op in non-shell environments, when standard
 * API exists, or when native says unsupported.
 *
 * Swallow all exceptions: if the plugin hasn't synced into the native build,
 * `isSupported()` throws, and that shouldn't crash startup — worst case, we
 * get the pre-fallback behavior.
 */
export async function installNativeWakeLock(): Promise<void> {
  if (!Capacitor.isNativePlatform()) return;
  const std =
    typeof navigator === "undefined"
      ? undefined
      : (navigator as unknown as { wakeLock?: unknown }).wakeLock;
  if (std) return;
  try {
    const { isSupported } = await KeepAwake.isSupported();
    if (!isSupported) return;
    setWakeLockFallback(nativeWakeLock());
  } catch {
    /* Plugin not installed — treat as no keep-awake capability */
  }
}
