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
export type WakeLockSentinelLike = {
  released: boolean;
  release: () => Promise<void>;
  addEventListener: (type: "release", listener: () => void) => void;
};
export type WakeLockLike = { request: (type: "screen") => Promise<WakeLockSentinelLike> };

/**
 * WebView 没有标准 API 时的兜底实现（iOS 18.4 以下走原生 keep-awake）。
 *
 * 由 wakeLockNative.ts 在启动时装进来 —— 本模块**不**认识 Capacitor：这样它在
 * 纯浏览器和 node 测试里都不会拖进一个原生依赖，鸿蒙那条早就存在的
 * `navigator.wakeLock` 垫片也照旧走标准路径，三种环境共用同一套持锁逻辑。
 */
let fallback: WakeLockLike | null = null;

function api(): WakeLockLike | undefined {
  const std =
    typeof navigator === "undefined"
      ? undefined
      : (navigator as unknown as { wakeLock?: WakeLockLike }).wakeLock;
  return std ?? fallback ?? undefined;
}

/** 装一条兜底实现；标准 API 在场时它不会被用到。 */
export function setWakeLockFallback(impl: WakeLockLike | null): void {
  fallback = impl;
  sync();
  // supported 变了，设置页那个开关要跟着出现/消失。
  for (const fn of listeners) fn();
}

function visible(): boolean {
  return typeof document !== "undefined" && document.visibilityState === "visible";
}

function initialEnabled(): boolean {
  if (typeof localStorage === "undefined") return false;
  return localStorage.getItem(KEY) === "1";
}

let enabled = initialEnabled();
/** 临时持锁的引用计数（录音等「这段时间不能息屏」的场景）。 */
let holds = 0;
let sentinel: WakeLockSentinelLike | null = null;
const listeners = new Set<() => void>();

/**
 * 这一刻到底该不该持锁 —— 用户的常亮开关，**或**任何一个临时持锁。
 *
 * 两者故意分开存：临时持锁不能改写用户在设置里的选择，否则录一次音就把开关
 * 掰开了，录完也没人给他掰回去。
 */
function wanted(): boolean {
  return enabled || holds > 0;
}

/**
 * 已经发出、还没回来的那次 request。
 *
 * `sentinel` 要等 await 之后才赋值，光靠它挡不住**同一拍里**发起的第二次
 * acquire —— 引用计数持锁天然会这样（一次录音里两处同时 hold），结果是拿到
 * 两把锁而只记住一把，另一把再也没人释放：屏幕从此常亮不灭。
 */
let acquiring = false;

async function acquire(): Promise<void> {
  const wl = api();
  if (!wanted() || sentinel || acquiring || !wl || !visible()) return;
  acquiring = true;
  try {
    const s = await wl.request("screen");
    // 申请是异步的：如果 await 期间开关被关掉了，立刻释放，别留下幽灵锁。
    if (!wanted()) {
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
  } finally {
    acquiring = false;
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

/** 把实际持锁状态对齐到 enabled / 临时持锁 + 可见性。 */
function sync(): void {
  if (wanted()) void acquire();
  else void drop();
}

/**
 * 在一段时间内强制常亮，不理会用户的开关，返回释放函数。
 *
 * 用在语音输入这种「中途息屏 = 这次操作直接废掉」的场景：手机默认半分钟到一分钟
 * 就自动锁屏，而说一段长话很容易超过它 —— 屏一灭，WebView 被挂起，识别会话当场
 * 断掉，用户说的话全丢，还看不出发生了什么。
 *
 * 引用计数而不是布尔：可能同时有多处持有（录音条 + 别处），谁先放开都不该把别人
 * 的那一份也放掉。返回的函数幂等，重复调用只算一次。
 */
export function holdWakeLock(): () => void {
  holds++;
  sync();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    holds = Math.max(0, holds - 1);
    sync();
  };
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
  // supported 也进快照：兜底是启动后异步装上的，不带上它，设置页不会重画那个开关。
  return `${enabled ? "1" : "0"}${api() ? "s" : "-"}`;
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
