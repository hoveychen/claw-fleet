import type { ViewMode } from "../store";

/** Rail = the collapsible 二级侧边栏 a page hangs beside its main container. */
export interface RailConfig {
  /** Persisted width. MUST also be listed in storage.ts's ALL_KEYS, or the width
   *  is written on drag and silently not read back on the next boot. */
  storageKey: string;
  min: number;
  max: number;
  initial: number;
  /** Which edge the rail hugs. Audit is the only right-hand one: its list is the
   *  main container and the detail pane is the rail. Fixed per page rather than
   *  passed at the call site — Audit renders its shell from two different tabs,
   *  and a call-site parameter is how those two drift apart. */
  side?: "left" | "right";
}

/** The single source of truth for "which pages have a secondary sidebar".
 *
 *  SECONDARY_SIDEBAR_VIEWS is DERIVED from this map, so "has a rail config" and
 *  "re-clicking the nav item collapses it" cannot disagree — they used to be two
 *  hand-maintained lists. */
export const RAILS: Partial<Record<ViewMode, RailConfig>> = {
  history: { storageKey: "history-rail-width", min: 240, max: 640, initial: 300 },
  audit: { storageKey: "audit-rail-width", min: 280, max: 720, initial: 380, side: "right" },
  memory: { storageKey: "memory-rail-width", min: 200, max: 640, initial: 340 },
  wiki: { storageKey: "wiki-rail-width", min: 170, max: 420, initial: 240 },
  skills: { storageKey: "skills-rail-width", min: 200, max: 640, initial: 340 },
  files: { storageKey: "files-rail-width", min: 200, max: 640, initial: 340 },
  plugins: { storageKey: "plugins-rail-width", min: 200, max: 640, initial: 340 },
  report: { storageKey: "report-rail-width", min: 240, max: 640, initial: 360 },
};

/** Views whose page carries a collapsible secondary sidebar. Re-clicking the
 *  already-active nav item toggles it; views without one just switch on click. */
export const SECONDARY_SIDEBAR_VIEWS: ReadonlySet<ViewMode> = new Set(
  Object.keys(RAILS) as ViewMode[],
);
