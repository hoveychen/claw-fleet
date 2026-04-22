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
  "tts-mode",
  "chime-sound",
  "tts-voice",
  "user-title",
  "auto-update-check",
  "llm-provider",
  "llm-model-fast",
  "llm-model-standard",
  "guard-enabled",
  "guard-llm-analysis",
  "elicitation-enabled",
  "onboarding-seen-features",
] as const;

// ── Onboarding feature registry ─────────────────────────────────────────────
// Each ID represents a configurable feature card in onboarding.
// Adding a new ID here will trigger a "What's New" overlay for existing users.
export const ONBOARDING_FEATURES = [
  "appearance",
  "notifications",
  "hooks_guard_elicitation",
  "global_ask",
] as const;

export type OnboardingFeatureId = (typeof ONBOARDING_FEATURES)[number];

/** Get the set of feature IDs the user has already seen. */
export function getSeenFeatures(): Set<OnboardingFeatureId> {
  const raw = getItem("onboarding-seen-features");
  if (!raw) return new Set();
  try {
    const arr = JSON.parse(raw);
    return new Set(arr as OnboardingFeatureId[]);
  } catch {
    return new Set();
  }
}

/** Mark a set of feature IDs as seen. Merges with existing. */
export function markFeaturesSeen(ids: OnboardingFeatureId[]): void {
  const existing = getSeenFeatures();
  for (const id of ids) existing.add(id);
  setItem("onboarding-seen-features", JSON.stringify([...existing]));
}

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
