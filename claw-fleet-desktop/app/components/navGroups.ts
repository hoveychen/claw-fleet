import type { ViewMode } from "../viewModes";

/** The sidebar nav has two levels. The pages reached for while an agent works
 *  sit at the top level; schedules, plan trees and the monitoring /
 *  administration pages (audit, the daily report, memory rules, skills, phone
 *  pairing) live on a "More" sub-page reached from the bottom of the list. */

/** Pages listed under "More" — the single source of truth. Every other
 *  {@link ViewMode} is a top-level item; navGroups.test.ts asserts the split. */
export const NAV_MORE_VIEWS: readonly ViewMode[] = [
  "schedule",
  "plans",
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

/** True when `view` is listed under "More". The nav shows the More sub-page
 *  while such a page is on screen, so a cross-page hop that bypasses the nav (an
 *  audit link, a schedule jump) lands with its nav item in view. */
export function isInNavMore(view: ViewMode): boolean {
  return MORE_SET.has(view);
}
