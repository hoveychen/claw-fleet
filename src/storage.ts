/**
 * Thin wrapper around @tauri-apps/plugin-store providing synchronous reads
 * and fire-and-forget async writes.
 *
 * On boot we load every key into an in-memory cache so that zustand stores and
 * i18n can read initial values synchronously.
 */

import { load, type Store } from "@tauri-apps/plugin-store";

let store: Store | null = null;
const cache = new Map<string, string>();

/** Keys we persist — add new ones here. */
const ALL_KEYS = [
  "theme",
  "viewMode",
  "lang",
  "sidebar-width",
  "onboarding-dismissed",
  "wizard-completed",
  "hooks-banner-dismissed",
  "notification-mode",
  "personalized-mascot",
  "overlay-enabled",
  "tts-mode",
  "chime-sound",
  "tts-voice",
  "overlay-muted",
  "user-title",
] as const;

/**
 * Must be called (and awaited) once before any get/set.
 * Typically in main.tsx before React renders.
 */
export async function initStorage(): Promise<void> {
  store = await load("settings.json", { defaults: {}, autoSave: true });
  for (const key of ALL_KEYS) {
    const val = await store.get<string>(key);
    if (val !== null && val !== undefined) {
      cache.set(key, val);
    }
  }
}

/** Synchronous read from in-memory cache. */
export function getItem(key: string): string | null {
  return cache.get(key) ?? null;
}

/** Write to both cache and Tauri store (async, fire-and-forget). */
export function setItem(key: string, value: string): void {
  cache.set(key, value);
  store?.set(key, value);
}
