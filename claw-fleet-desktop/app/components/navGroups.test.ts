import { describe, expect, it } from "vitest";
import { NAV_HOME, NAV_MORE_VIEWS, isInNavMore } from "./navGroups";
import { ALL_VIEW_MODES } from "../viewModes";

/**
 * The sidebar lists the work pages at the top level and puts the rest on a
 * "More" sub-page. The nav enters that sub-page while one of its pages is on
 * screen, which only works if the "More" table names real pages.
 */
describe("nav More split", () => {
  it("lists only real view modes, each once", () => {
    expect(new Set(NAV_MORE_VIEWS).size).toBe(NAV_MORE_VIEWS.length);
    for (const view of NAV_MORE_VIEWS) {
      expect(ALL_VIEW_MODES).toContain(view);
    }
  });

  it("puts schedules, plan trees and the monitoring / management pages under More", () => {
    for (const view of ["schedule", "plans", "audit", "report", "memory", "skills", "plugins", "mobile"] as const) {
      expect(isInNavMore(view)).toBe(true);
    }
  });

  it("keeps the agent-work pages at the top level", () => {
    for (const view of ["history", "files", "terminal", "wiki", "artifacts"] as const) {
      expect(isInNavMore(view)).toBe(false);
    }
  });

  it("lands home on a top-level page", () => {
    expect(isInNavMore(NAV_HOME)).toBe(false);
  });
});
