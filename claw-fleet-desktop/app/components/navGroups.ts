import type { ViewMode } from "../viewModes";

/** The two top-level modes the sidebar tab strip switches between.
 *  - `fleet` (舰队): watching and administering the fleet — sessions, audit,
 *    the daily report, memory rules, skills, phone pairing.
 *  - `work` (工作): what you reach for while an agent is actually working —
 *    tasks, repos, the wiki, deliverables, schedules, plan trees. */
export type NavGroup = "fleet" | "work";

/** Tab order in the strip. */
export const NAV_GROUPS: readonly NavGroup[] = ["fleet", "work"];

/** Which pages hang under each tab — the single source of truth. The nav
 *  renders its items from this map and {@link navGroupOf} is derived from it, so
 *  "which tab lists this page" and "which tab lights up while that page is
 *  open" cannot drift apart. Every {@link ViewMode} must appear exactly once;
 *  navGroups.test.ts asserts the partition. */
export const NAV_GROUP_VIEWS: Record<NavGroup, readonly ViewMode[]> = {
  fleet: ["list", "gallery", "audit", "report", "memory", "skills", "plugins", "mobile"],
  work: ["history", "files", "terminal", "wiki", "artifacts", "schedule", "plans"],
};

/** Where a tab lands when it has no remembered last page (first click ever, or
 *  a stored value that no longer belongs to the group). */
export const NAV_GROUP_HOME: Record<NavGroup, ViewMode> = {
  fleet: "gallery",
  work: "history",
};

const GROUP_BY_VIEW: ReadonlyMap<ViewMode, NavGroup> = new Map(
  NAV_GROUPS.flatMap((group) =>
    NAV_GROUP_VIEWS[group].map((view) => [view, group] as const),
  ),
);

/** Which tab owns `view`. The active tab is *derived* from the current viewMode
 *  rather than stored alongside it: cross-page hops that bypass the nav
 *  (AuditView → sessions, the wiki `[[slug]]` mention jump, Wizard → tasks, a
 *  tray click → tasks) then move the tab for free instead of leaving it
 *  pointing at a group that isn't on screen. Falls back to the first group so
 *  an unmapped view still renders something. */
export function navGroupOf(view: ViewMode): NavGroup {
  return GROUP_BY_VIEW.get(view) ?? NAV_GROUPS[0];
}

/** True when `view` belongs to `group`. */
export function isInNavGroup(view: ViewMode, group: NavGroup): boolean {
  return navGroupOf(view) === group;
}

/** Guard for values read back from storage. */
export function isNavGroup(value: unknown): value is NavGroup {
  return NAV_GROUPS.includes(value as NavGroup);
}
