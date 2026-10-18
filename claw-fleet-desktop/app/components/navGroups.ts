import type { ViewMode } from "../viewModes";

/** The sidebar has four main entries: Tasks (home), Library, Projects and
 *  Settings, plus one secondary entry beside Settings: Review (回顾, the
 *  per-project daily progress that replaced the daily report). Every page
 *  belongs to one of them; a section that holds more than one page shows the
 *  others as a segmented switch in the page banner (see `useSectionTabs` in
 *  PageShell). */
export type NavSection = "tasks" | "library" | "projects" | "review" | "settings";

/** The main entries, top to bottom. */
export const NAV_SECTIONS: readonly NavSection[] = ["tasks", "library", "projects", "settings"];

/** Main entries plus the secondary one — every section a page can belong to. */
export const ALL_NAV_SECTIONS: readonly NavSection[] = [...NAV_SECTIONS, "review"];

/** Pages of each section, in the order the section switch draws them. The
 *  first page is where a click on the section's nav entry lands the first time;
 *  after that the entry returns to whichever page of it was open last.
 *  navGroups.test.ts asserts every {@link ViewMode} is listed exactly once. */
export const NAV_SECTION_VIEWS: Record<NavSection, readonly ViewMode[]> = {
  tasks: ["history"],
  library: ["library", "wiki", "artifacts"],
  projects: ["projects", "files", "terminal", "memory"],
  review: ["report"],
  // Reached from the Settings overlay ("more pages"); the nav entry itself
  // opens the overlay. The lobster fleet is an opt-in easter egg launched from
  // its own Settings toggle, never from the nav.
  settings: ["audit", "skills", "plugins", "mobile", "lobsters"],
};

/** Where the app lands when nothing else picks a page. */
export const NAV_HOME: ViewMode = "history";

const SECTION_OF = new Map<ViewMode, NavSection>(
  ALL_NAV_SECTIONS.flatMap((section) => NAV_SECTION_VIEWS[section].map((v) => [v, section] as const)),
);

/** Which nav entry a page lights up. */
export function navSectionOf(view: ViewMode): NavSection {
  return SECTION_OF.get(view) ?? "tasks";
}

/** Pages that exist only on some hosts. Terminal needs the backend's
 *  FLEET_TERMINAL; the phone pairing page only where a phone can reach this
 *  host; the lobster fleet only once its Settings toggle is on. */
export interface NavAvailability {
  terminal: boolean;
  mobile: boolean;
  lobsters: boolean;
}

const ALL_AVAILABLE: NavAvailability = { terminal: true, mobile: true, lobsters: true };

export function isViewAvailable(view: ViewMode, avail: NavAvailability): boolean {
  if (view === "terminal") return avail.terminal;
  if (view === "mobile") return avail.mobile;
  if (view === "lobsters") return avail.lobsters;
  return true;
}

/** The page a section's nav entry opens: the last page of that section the
 *  user was on, else the section's first page this host has. */
export function navSectionTarget(
  section: NavSection,
  lastBySection: Partial<Record<NavSection, ViewMode>>,
  avail: NavAvailability = ALL_AVAILABLE,
): ViewMode {
  const last = lastBySection[section];
  if (last && navSectionOf(last) === section && isViewAvailable(last, avail)) return last;
  return NAV_SECTION_VIEWS[section].find((v) => isViewAvailable(v, avail)) ?? NAV_HOME;
}

/** Pages that do not get a segment of their own in the section switch, mapped
 *  to the segment that stands for them. Plugins are a source of skills and sit
 *  behind the Skills page's own source tabs (SkillsSourceTabs). */
const TAB_ALIAS: Partial<Record<ViewMode, ViewMode>> = { plugins: "skills" };

/** The segment `view` lights up in its section switch. */
export function navTabOf(view: ViewMode): ViewMode {
  return TAB_ALIAS[view] ?? view;
}

/** Segments of a section's switch, in order. The lobster fleet is an easter
 *  egg opened from Settings and never offered as a segment. */
export function navSectionTabs(section: NavSection, avail: NavAvailability): ViewMode[] {
  return NAV_SECTION_VIEWS[section].filter(
    (v) => v !== "lobsters" && !TAB_ALIAS[v] && isViewAvailable(v, avail),
  );
}
