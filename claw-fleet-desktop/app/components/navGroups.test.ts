import { describe, expect, it } from "vitest";
import {
  NAV_HOME,
  NAV_SECTIONS,
  NAV_SECTION_VIEWS,
  navSectionOf,
  navSectionTarget,
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

  it("files documents and deliverables under Library, code and routines under Projects", () => {
    expect(navSectionOf("wiki")).toBe("library");
    expect(navSectionOf("artifacts")).toBe("library");
    for (const v of ["files", "terminal", "schedule", "memory"] as const) {
      expect(navSectionOf(v)).toBe("projects");
    }
    expect(navSectionOf("lobsters")).toBe("settings");
  });

  it("lands home on the task page", () => {
    expect(NAV_HOME).toBe("history");
    expect(navSectionOf(NAV_HOME)).toBe("tasks");
  });

  it("returns to the last page of a section, else its first", () => {
    expect(navSectionTarget("library", {})).toBe("wiki");
    expect(navSectionTarget("library", { library: "artifacts" })).toBe("artifacts");
    // A stale entry naming a page of another section is ignored.
    expect(navSectionTarget("projects", { projects: "wiki" })).toBe("files");
  });
});
