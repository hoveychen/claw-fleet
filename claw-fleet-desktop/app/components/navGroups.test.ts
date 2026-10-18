import { describe, expect, it } from "vitest";
import {
  NAV_HOME,
  NAV_SECTIONS,
  NAV_SECTION_VIEWS,
  navSectionOf,
  navSectionTabs,
  navSectionTarget,
  navTabOf,
} from "./navGroups";
import { ALL_VIEW_MODES } from "../viewModes";

/**
 * The sidebar has four entries and every page hangs under exactly one of them.
 * A page added to ALL_VIEW_MODES and forgotten here would light up no nav entry
 * and be reachable from nowhere.
 */
describe("nav sections", () => {
  it("has the four entries in order", () => {
    expect(NAV_SECTIONS).toEqual(["tasks", "library", "projects", "settings"]);
  });

  it("assigns every view mode to exactly one section", () => {
    const listed = NAV_SECTIONS.flatMap((s) => NAV_SECTION_VIEWS[s]);
    expect(new Set(listed).size).toBe(listed.length);
    expect([...listed].sort()).toEqual([...ALL_VIEW_MODES].sort());
  });

  it("files documents and deliverables under Library, code and memory under Projects", () => {
    expect(navSectionOf("wiki")).toBe("library");
    expect(navSectionOf("artifacts")).toBe("library");
    for (const v of ["projects", "files", "terminal", "memory"] as const) {
      expect(navSectionOf(v)).toBe("projects");
    }
    expect(navSectionOf("lobsters")).toBe("settings");
  });

  it("lands home on the task page", () => {
    expect(NAV_HOME).toBe("history");
    expect(navSectionOf(NAV_HOME)).toBe("tasks");
  });

  it("returns to the last page of a section, else its first", () => {
    expect(navSectionTarget("library", {})).toBe("library");
    expect(navSectionTarget("library", { library: "artifacts" })).toBe("artifacts");
    // A stale entry naming a page of another section is ignored.
    expect(navSectionTarget("projects", { projects: "wiki" })).toBe("projects");
  });

  it("skips pages this host does not have", () => {
    const avail = { terminal: false, mobile: false, lobsters: false };
    expect(navSectionTarget("projects", { projects: "terminal" }, avail)).toBe("projects");
    expect(navSectionTabs("projects", avail)).toEqual(["projects", "files", "memory"]);
  });

  it("folds plugins into the Skills segment and never offers the lobster fleet", () => {
    const avail = { terminal: true, mobile: true, lobsters: true };
    expect(navSectionTabs("settings", avail)).toEqual(["audit", "report", "skills", "mobile"]);
    expect(navTabOf("plugins")).toBe("skills");
    expect(navTabOf("audit")).toBe("audit");
  });
});
