import type { ViewMode } from "../viewModes";

/** The sidebar is one nav list. The pages reached for while an agent works sit
 *  at the top level; the monitoring / administration pages (audit, the daily
 *  report, memory rules, skills, phone pairing) fold under a "More" disclosure
 *  at the bottom of the list. */

/** Pages listed under "More" — the single source of truth. Every other
 *  {@link ViewMode} is a top-level item; navGroups.test.ts asserts the split. */
export const NAV_MORE_VIEWS: readonly ViewMode[] = [
  "audit",
  "report",
  "memory",
  "skills",
  "plugins",
  "mobile",
];

/** Where the app lands when nothing else picks a page. */
export const NAV_HOME: ViewMode = "history";

const MORE_SET: ReadonlySet<ViewMode> = new Set(NAV_MORE_VIEWS);

/** True when `view` is listed under "More". The disclosure opens itself while
 *  such a page is on screen, so a cross-page hop that bypasses the nav (an audit
 *  link, a tray click) never leaves the active page hidden in a closed menu. */
export function isInNavMore(view: ViewMode): boolean {
  return MORE_SET.has(view);
}
