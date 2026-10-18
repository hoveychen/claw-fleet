import type { ViewMode } from "../viewModes";

/** The sidebar has exactly four entries: Tasks (home), Library, Projects and
 *  Settings. Every page belongs to one of them; a section that holds more than
 *  one page shows the others as a segmented switch in the page banner (see
 *  `NavSectionTabs` in PageShell). */
export type NavSection = "tasks" | "library" | "projects" | "settings";

export const NAV_SECTIONS: readonly NavSection[] = ["tasks", "library", "projects", "settings"];

/** Pages of each section, in the order the section switch draws them. The
 *  first page is where a click on the section's nav entry lands the first time;
 *  after that the entry returns to whichever page of it was open last.
 *  navGroups.test.ts asserts every {@link ViewMode} is listed exactly once. */
export const NAV_SECTION_VIEWS: Record<NavSection, readonly ViewMode[]> = {
  tasks: ["history"],
  library: ["wiki", "artifacts"],
  projects: ["files", "terminal", "schedule", "memory"],
  // Reached from the Settings overlay ("more pages"); the nav entry itself
  // opens the overlay. The lobster fleet is an opt-in easter egg launched from
  // its own Settings toggle, never from the nav.
  settings: ["audit", "report", "skills", "plugins", "mobile", "lobsters"],
};

/** Where the app lands when nothing else picks a page. */
export const NAV_HOME: ViewMode = "history";

const SECTION_OF = new Map<ViewMode, NavSection>(
  NAV_SECTIONS.flatMap((section) => NAV_SECTION_VIEWS[section].map((v) => [v, section] as const)),
);

/** Which nav entry a page lights up. */
export function navSectionOf(view: ViewMode): NavSection {
  return SECTION_OF.get(view) ?? "tasks";
}

/** The page a section's nav entry opens: the last page of that section the
 *  user was on, else the section's first page. */
export function navSectionTarget(section: NavSection, lastBySection: Partial<Record<NavSection, ViewMode>>): ViewMode {
  const last = lastBySection[section];
  if (last && navSectionOf(last) === section) return last;
  return NAV_SECTION_VIEWS[section][0];
}
