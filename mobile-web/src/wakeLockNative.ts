// 原生的屏幕常亮兜底 —— Capacitor 壳专用。
//
// 为什么需要它：iOS 的 WKWebView 到 **18.4** 才有 Screen Wake Lock，16.4–18.3 那版
// 在独立 Web App 里根本不工作（WebKit bug 254545）。也就是说 wakeLock.ts 那条标准
// 路径在相当一部分在役 iPhone 上是空转的 —— 而语音输入正好是「中途息屏 = 这次
// 白说」的场景，空转等于没修。
//
// @capacitor-community/keep-awake 底下就是 iOS 的 `UIApplication.isIdleTimerDisabled`
// 和 Android 的 `FLAG_KEEP_SCREEN_ON`。选它而不是自己写 plugin 的理由和
// voiceCapacitor.ts 挑 @capgo 那个包一样：自己写要维护 Swift + Kotlin 两套原生代码。
//
// 它只在**壳里、且没有标准 API 时**装上：Android WebView（84+）和 iOS 18.4+ 都自带
// navigator.wakeLock，鸿蒙的 WebShell 也自己注入了一个垫片，那些环境一律走标准路径。

import { Capacitor } from "@capacitor/core";
import { KeepAwake } from "@capacitor-community/keep-awake";
import { setWakeLockFallback, type WakeLockLike, type WakeLockSentinelLike } from "./wakeLock";

/** 把 keep-awake 包装成 wakeLock.ts 认识的形状。 */
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
        // 原生这条路没有「系统主动收走」的事件（切后台由 wakeLock.ts 的
        // visibilitychange 兜着），所以这里没有可转发的东西。
        addEventListener() {},
      };
      return sentinel;
    },
  };
}

/**
 * 启动时调一次。非壳环境、已有标准 API、或原生说不支持时都静默不装。
 *
 * 全程吞异常：壳里插件没同步进原生工程时 `isSupported()` 会抛，那不该炸掉启动 ——
 * 最坏的结果是回到没有这条兜底之前的样子。
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
    /* 插件没装好 —— 当作没有常亮能力 */
  }
}
