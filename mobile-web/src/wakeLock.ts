// 屏幕常亮：Web Screen Wake Lock API。开启后在前台持有一个 sentinel，
// 阻止移动端自动息屏——盯着某个 session 实时跑、或等决策卡时不被打断。
//
// 关键约束：wake lock 在页面切到后台会被系统自动释放，回到前台不会自动
// 恢复。所以这里在模块级挂一个 visibilitychange 监听，回前台且开关仍开时
// 重新 acquire。持久化沿用 theme.ts / i18n.ts 的 localStorage + 自研
// useSyncExternalStore store 套路。

import { useSyncExternalStore } from "react";

const KEY = "fleet-wake-lock";

// lib.dom 对 WakeLock 的类型覆盖各版本不一致，这里像 push.ts 一样用最小接口
// 自描述并 unknown 转型，避免依赖 lib 版本。
type WakeLockSentinelLike = {
  released: boolean;
  release: () => Promise<void>;
  addEventListener: (type: "release", listener: () => void) => void;
};
type WakeLockLike = { request: (type: "screen") => Promise<WakeLockSentinelLike> };

function api(): WakeLockLike | undefined {
  if (typeof navigator === "undefined") return undefined;
  return (navigator as unknown as { wakeLock?: WakeLockLike }).wakeLock;
}

function visible(): boolean {
  return typeof document !== "undefined" && document.visibilityState === "visible";
}

function initialEnabled(): boolean {
  if (typeof localStorage === "undefined") return false;
  return localStorage.getItem(KEY) === "1";
}

let enabled = initialEnabled();
let sentinel: WakeLockSentinelLike | null = null;
const listeners = new Set<() => void>();

async function acquire(): Promise<void> {
  const wl = api();
  if (!enabled || sentinel || !wl || !visible()) return;
  try {
    const s = await wl.request("screen");
    // 申请是异步的：如果 await 期间开关被关掉了，立刻释放，别留下幽灵锁。
    if (!enabled) {
      void s.release().catch(() => {});
      return;
    }
    sentinel = s;
    // 系统释放（切后台 / 低电量）时清引用，回前台好重新申请。
    s.addEventListener("release", () => {
      if (sentinel === s) sentinel = null;
    });
  } catch {
    // 不在前台 / 浏览器策略拒绝——静默，回前台或再点开关时会补上。
    sentinel = null;
  }
}

async function drop(): Promise<void> {
  const s = sentinel;
  sentinel = null;
  if (s && !s.released) {
    try {
      await s.release();
    } catch {
      /* 已被系统释放 */
    }
  }
}

/** 把实际持锁状态对齐到 enabled + 可见性。 */
function sync(): void {
  if (enabled) void acquire();
  else void drop();
}

// 回前台重拿（系统会在切后台时自动释放 sentinel）。模块级只注册一次。
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") sync();
  });
}

export function isWakeLockSupported(): boolean {
  return !!api();
}

export function getWakeLockEnabled(): boolean {
  return enabled;
}

export function setWakeLockEnabled(next: boolean): void {
  if (next === enabled) return;
  enabled = next;
  if (typeof localStorage !== "undefined") {
    localStorage.setItem(KEY, next ? "1" : "0");
  }
  sync();
  for (const fn of listeners) fn();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function snapshot(): string {
  return enabled ? "1" : "0";
}

export function useWakeLock(): {
  supported: boolean;
  enabled: boolean;
  setEnabled: (v: boolean) => void;
} {
  useSyncExternalStore(subscribe, snapshot);
  return { supported: isWakeLockSupported(), enabled, setEnabled: setWakeLockEnabled };
}

/**
 * main.tsx 启动时调用：若上次开着就尝试恢复。首帧无 user gesture 时部分浏览器
 * 可能拒绝，但页面本就可见，多数浏览器（含 iOS 16.4+ / 安卓 Chrome）允许；
 * 万一失败，回前台的 visibilitychange 或用户再点开关都会补上。
 */
export function initWakeLock(): void {
  sync();
}
