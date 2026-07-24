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

/** Keys we persist — add new ones here.
 *
 *  This list is also the *boot preload* list: `initStorage` only pulls these
 *  keys into the synchronous cache, so a key written with `setItem` but missing
 *  here is saved to disk and then never read back — `getItem` returns null on
 *  the next launch and whatever it feeds silently reverts to its default. Any
 *  new `setItem` key MUST be registered here. */
const ALL_KEYS = [
  "theme",
  "viewMode",
  // Which session sub-view (list vs gallery) the unified "Sessions" nav returns to.
  "lastSessionViewMode",
  // One-shot flag: existing users' stored "list" has been flipped to the new
  // gallery default (see migrateSessionViewDefault). Must be readable on boot
  // so the migration never re-runs and clobbers a later deliberate "list".
  "gallery-default-migrated",
  // One-shot flag: legacy binary values for the tristate-migrated feature keys
  // have been reset to "default" (see migrateFeatureTristate). MUST be readable
  // on boot, otherwise the migration re-runs every launch and re-wipes whatever
  // the user has since chosen.
  "feature-tristate-migrated",
  "liteMode",
  "lang",
  "sidebar-width",
  "sidebar-collapsed",
  // Per-view collapse map for the secondary sidebars, stored as a JSON blob.
  "secondary-sidebar-collapsed",
  // Secondary sidebar (二级侧边栏) widths — one per view that owns one.
  "history-rail-width",
  "memory-rail-width",
  "wiki-rail-width",
  "skills-rail-width",
  "files-rail-width",
  "plugins-rail-width",
  "audit-rail-width",
  "report-rail-width",
  // File-tree columns inside a detail pane (SkillsView / FilesView / ScratchpadView).
  "skills-tree-width",
  "files-tree-width",
  // Commands pinned to the top of the repository command runner.
  "proc-shortcuts",
  "scratchpad-tree-width",
  // 启动台 (HistoryView) rail filters — the search box is deliberately absent,
  // it stays in-memory only.
  "history-mark-filter",
  "history-workspace-filter",
  "history-active-only",
  "history-group-handoff",
  "onboarding-dismissed",
  "wizard-completed",
  "hooks-banner-dismissed",
  "notification-mode",
  "personalized-mascot",
  "mascot-visible",
  "tts-mode",
  "chime-sound",
  "tts-voice",
  "tts-muted",
  "user-title",
  "auto-update-check",
  "llm-provider",
  "llm-model-fast",
  "llm-model-standard",
  "guard-enabled",
  "guard-llm-analysis",
  "elicitation-enabled",
  // Onboarding toggles whose checkbox state is read back through getItem. The
  // features themselves live in ~/.claude hook config; these only drive the UI.
  "interaction-mode-enabled",
  "prd-mode-enabled",
  "wiki-guidance-enabled",
  "model-guidance-enabled",
  "plan-approval-enabled",
  "onboarding-seen-features",
  "usage-auto-refresh",
  // DecisionPanel presentation.
  "decision-panel-collapsed",
  "floating-decision-panel",
  // Read-state for audit entries, stored as a JSON blob.
  "audit-read-keys",
  // The most recent daily-report date the user has viewed (YYYY-MM-DD). Drives
  // the "new report" red dot on the 每日报告 nav item.
  "daily-report-last-seen",
  // The 任务 (HistoryView) detail column's open tabs: {tabIds, activeId}.
  // Restored on boot and pruned against the first scan, so ids of sessions that
  // have since been deleted drop out instead of accumulating forever.
  "launchpad-tabs",
] as const;

// ── Onboarding feature registry ─────────────────────────────────────────────
// Each ID represents a configurable feature card in onboarding.
// Adding a new ID here will trigger a "What's New" overlay for existing users.
export const ONBOARDING_FEATURES = [
  "appearance",
  "notifications",
  "hooks_guard_elicitation",
  "global_ask",
  "prd_discipline",
  "wiki_guidance",
  "model_guidance",
  "skill_interop",
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

/**
 * One-time rollout of gallery as the default session view. The code default is
 * already "gallery", but existing users can carry a stored "list" — most often
 * written accidentally by an older notification/tray path that force-switched
 * to list and persisted it. Flip a stored "list" to "gallery" exactly once,
 * guarded by a persisted flag so a later *deliberate* toggle back to list still
 * sticks. Call once on boot, after initStorage() and before the store reads.
 */
export function migrateSessionViewDefault(): void {
  if (getItem("gallery-default-migrated") === "true") return;
  if (getItem("viewMode") === "list") setItem("viewMode", "gallery");
  if (getItem("lastSessionViewMode") === "list") {
    setItem("lastSessionViewMode", "gallery");
  }
  setItem("gallery-default-migrated", "true");
}

/** Write to both cache and Tauri store (async, fire-and-forget). */
export function setItem(key: string, value: string): void {
  cache.set(key, value);
  store?.set(key, value);
}

/** Delete from both cache and Tauri store (async, fire-and-forget). Used by the
 *  tristate "默认/default" state, which is represented by the ABSENCE of a
 *  stored value so the feature follows FEATURE_DEFAULTS. */
export function removeItem(key: string): void {
  cache.delete(key);
  store?.delete(key);
}

// ── Tristate feature toggles (on / off / default) ───────────────────────────
//
// Every boolean feature toggle is tristate. A stored value is ONLY ever written
// when the user makes an explicit choice ("on" / "off"); the "default" state is
// the ABSENCE of a stored value, which makes the feature follow the single
// source of truth below. Changing a feature's recommended default is therefore
// a one-line edit to FEATURE_DEFAULTS — every user still on "default" follows it
// automatically, with NO migration ever needed again.
//
// (Legacy "true"/"false" values written before the tristate refactor are still
// understood by the readers below, so nothing breaks during the transition; the
// one-time migrateFeatureTristate() normalizes the keys whose default changed.)
export const FEATURE_DEFAULTS: Record<string, boolean> = {
  // Default ON.
  "guard-enabled": true,
  "guard-llm-analysis": true,
  "elicitation-enabled": true,
  "interaction-mode-enabled": true,
  "plan-approval-enabled": true,
  "prd-mode-enabled": true,
  "wiki-guidance-enabled": true,
  "model-guidance-enabled": true,
  "auto-update-check": true,
  // Default OFF.
  "tts-muted": false,
  "personalized-mascot": false,
  "mascot-visible": false,
  "floating-decision-panel": false,
  "skill-autosync-enabled": false,
};

export type FeatureState = "on" | "off" | "default";

/** The recommended default (from FEATURE_DEFAULTS) for a feature key. */
export function featureDefault(key: string): boolean {
  return FEATURE_DEFAULTS[key] ?? false;
}

/** Resolve a tristate feature key to a concrete boolean: the user's explicit
 *  choice if any, otherwise the central default. Accepts both the canonical
 *  "on"/"off" and the legacy "true"/"false" forms. */
export function resolveFeature(key: string): boolean {
  const v = getItem(key);
  if (v === "on" || v === "true") return true;
  if (v === "off" || v === "false") return false;
  return featureDefault(key);
}

/** Resolve a tristate UI state to a concrete boolean given its key: the explicit
 *  "on"/"off" choice, or the central default for "default". Companion to
 *  resolveFeature() for callers that already hold the FeatureState in component
 *  state and want to avoid a second storage read. */
export function resolveFeatureState(state: FeatureState, key: string): boolean {
  if (state === "on") return true;
  if (state === "off") return false;
  return featureDefault(key);
}

/** The tristate UI state of a feature key: the user's explicit choice, or
 *  "default" when no value is stored. */
export function getFeatureState(key: string): FeatureState {
  const v = getItem(key);
  if (v === "on" || v === "true") return "on";
  if (v === "off" || v === "false") return "off";
  return "default";
}

/** Persist a tristate choice. "default" clears the key so the feature follows
 *  FEATURE_DEFAULTS (and any future change to it). */
export function setFeatureState(key: string, state: FeatureState): void {
  if (state === "default") removeItem(key);
  else setItem(key, state);
}

// Keys whose recommended default changed in the "default all features ON" work.
// Existing users can carry a stale binary value here — most often written
// automatically by the old SettingsPanel mount reconciliation (which persisted
// "false" whenever the sentinel wasn't yet installed), not by a deliberate user
// choice. Reset ALL of them to "default" once so those users follow the new
// central default. (Boss decision 2026-07-24: reset all old values, accepting
// that the rare user who deliberately set one will have it re-defaulted once.)
const TRISTATE_MIGRATION_KEYS = [
  "interaction-mode-enabled",
  "plan-approval-enabled",
  "prd-mode-enabled",
  "wiki-guidance-enabled",
  "model-guidance-enabled",
  "tts-mode",
];

/**
 * One-time reset of the changed-default feature keys to the "default" state
 * (absence), so existing users follow FEATURE_DEFAULTS / the mode defaults.
 * Guarded by a persisted flag so a later *deliberate* choice still sticks.
 * Call once on boot, after initStorage() and before the store reads.
 */
export function migrateFeatureTristate(): void {
  if (getItem("feature-tristate-migrated") === "true") return;
  for (const key of TRISTATE_MIGRATION_KEYS) removeItem(key);
  setItem("feature-tristate-migrated", "true");
}
