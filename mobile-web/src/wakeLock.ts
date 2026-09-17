// Screen wake lock: Web Screen Wake Lock API. When enabled, holds a sentinel
// in the foreground to prevent the mobile screen from auto-dimming—allows
// uninterrupted viewing of a session in real time or waiting for a decision card.
//
// Key constraint: wake lock is automatically released by the system when the page
// goes to the background and is not auto-restored on returning to foreground.
// So this module registers a visibilitychange listener at module level to
// re-acquire when returning to foreground if the toggle is still on. Persistence
// uses the same localStorage + custom useSyncExternalStore pattern as theme.ts / i18n.ts.

import { useSyncExternalStore } from "react";

const KEY = "fleet-wake-lock";

// lib.dom's type coverage for WakeLock varies across versions. Like push.ts,
// we use a minimal self-documenting interface and cast via unknown to avoid lib version dependency.
export type WakeLockSentinelLike = {
  released: boolean;
  release: () => Promise<void>;
  addEventListener: (type: "release", listener: () => void) => void;
};
export type WakeLockLike = { request: (type: "screen") => Promise<WakeLockSentinelLike> };

/**
 * Fallback implementation when WebView lacks a standard API (iOS below 18.4 uses native keep-awake).
 *
 * Injected by wakeLockNative.ts at startup. This module does **not** know about Capacitor,
 * so it won't pull in a native dependency in pure browser or node tests. HarmonyOS's existing
 * `navigator.wakeLock` shim continues via the standard path, and all three environments
 * share the same lock-holding logic.
 */
let fallback: WakeLockLike | null = null;

function api(): WakeLockLike | undefined {
  const std =
    typeof navigator === "undefined"
      ? undefined
      : (navigator as unknown as { wakeLock?: WakeLockLike }).wakeLock;
  return std ?? fallback ?? undefined;
}

/** Install a fallback implementation; unused when the standard API is available. */
export function setWakeLockFallback(impl: WakeLockLike | null): void {
  fallback = impl;
  sync();
  // Support changed; the toggle in settings should show/hide accordingly.
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
/** Reference count for temporary hold (scenarios like recording where "screen must not dim for this duration"). */
let holds = 0;
let sentinel: WakeLockSentinelLike | null = null;
const listeners = new Set<() => void>();

/**
 * Whether we should hold the lock right now: user's keep-awake toggle **or** any temporary hold.
 *
 * Intentionally kept separate: temporary holds must not overwrite the user's settings choice,
 * otherwise recording once would turn the toggle on and never turn it back off.
 */
function wanted(): boolean {
  return enabled || holds > 0;
}

/**
 * A request that has been issued but not yet returned.
 *
 * `sentinel` only gets assigned after await, so it alone cannot prevent **concurrent** acquire calls
 * in the same tick. Reference-counted holds naturally exhibit this (two concurrent hold() calls during
 * recording), resulting in acquiring two locks but only remembering one—the other is never released,
 * leaving the screen permanently awake.
 */
let acquiring = false;

async function acquire(): Promise<void> {
  const wl = api();
  if (!wanted() || sentinel || acquiring || !wl || !visible()) return;
  acquiring = true;
  try {
    const s = await wl.request("screen");
    // Request is async: if the toggle is turned off during await, release immediately to avoid orphaning the lock.
    if (!wanted()) {
      void s.release().catch(() => {});
      return;
    }
    sentinel = s;
    // When the system releases the lock (background/low battery), clear the reference so we can re-acquire on foreground.
    s.addEventListener("release", () => {
      if (sentinel === s) sentinel = null;
    });
  } catch {
    // Not in foreground / browser policy denied—silent failure; will retry on foreground or toggle change.
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
      /* Already released by system */
    }
  }
}

/** Align actual lock state to enabled / temporary hold + visibility. */
function sync(): void {
  if (wanted()) void acquire();
  else void drop();
}

/**
 * Force keep-awake for a duration, ignoring the user's toggle; returns a release function.
 *
 * Used for voice input where "screen dims mid-operation = operation fails": phones auto-lock
 * after 30 seconds to a minute by default, but speaking a long phrase easily exceeds that.
 * Screen dims → WebView suspends → recognition session breaks → all user input lost, with no visible error.
 *
 * Reference-counted rather than boolean: multiple holders may exist simultaneously (recording bar + elsewhere).
 * An early release should not drop the other's hold. The returned function is idempotent;
 * repeated calls count as one.
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

// Re-acquire on foreground (system auto-releases sentinel on background). Registered once at module level.
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
  // Include support in snapshot: fallback is installed async after startup, and without it the settings page won't repaint the toggle.
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
 * Called at main.tsx startup: if previously enabled, attempts to restore.
 * Some browsers may deny on the first frame without user gesture, but since the page
 * is already visible, most browsers (including iOS 16.4+ / Android Chrome) allow it.
 * If it fails, foreground visibilitychange or user toggle will retry.
 */
export function initWakeLock(): void {
  sync();
}
