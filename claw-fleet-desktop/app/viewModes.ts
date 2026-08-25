/**
 * The set of pages the main area can show.
 *
 * Its own module rather than a line in store.ts because both the store and the
 * sidebar's nav-group table (components/navGroups.ts) need it: the store asks
 * navGroups which tab a page belongs to, and navGroups asserts its table covers
 * every page. Keeping the enum here breaks what would otherwise be a cycle
 * between those two. `store.ts` re-exports these, so existing
 * `import type { ViewMode } from "./store"` call sites keep working.
 */

/** Runtime tuple, not a bare union: navGroups.test.ts enumerates it to prove
 *  every page is assigned to exactly one sidebar tab. A page added here and
 *  forgotten there fails that test instead of silently vanishing from the nav. */
export const ALL_VIEW_MODES = [
  "list",
  "gallery",
  "history",
  "audit",
  "report",
  "memory",
  "wiki",
  "skills",
  "plugins",
  "files",
  "mobile",
  "schedule",
  "plans",
] as const;

export type ViewMode = (typeof ALL_VIEW_MODES)[number];

export type SessionViewMode = Extract<ViewMode, "list" | "gallery">;

/** Guard for values read back from storage. */
export function isViewMode(value: unknown): value is ViewMode {
  return ALL_VIEW_MODES.includes(value as ViewMode);
}
