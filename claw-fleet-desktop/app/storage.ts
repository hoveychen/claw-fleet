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
