import { describe, expect, it, vi } from "vitest";
import {
  ALL_VIEW_MODES,
  NAV_GROUPS,
  NAV_GROUP_HOME,
  NAV_GROUP_VIEWS,
  isNavGroup,
  navGroupOf,
} from "./navGroups";

// navGroups imports ALL_VIEW_MODES from the store, which touches the tauri
// bridge at module load; stub it so this runs headless.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => {}),
}));

/**
 * The sidebar's 管家 / 工作 tab strip renders each group's nav items from
 * NAV_GROUP_VIEWS and derives the highlighted tab through navGroupOf. Those two
 * only stay in sync because the lookup map is built from the same table — and
 * because the table covers every page. A new ViewMode added to the store and
 * forgotten here would silently render in neither tab's nav while still being
 * reachable through a cross-page hop, leaving the strip pointing at 管家 with a
 * 工作 page on screen. These tests are that guard.
 */
describe("nav group partition", () => {
  it("assigns every view mode to exactly one group", () => {
    const assigned = NAV_GROUPS.flatMap((g) => [...NAV_GROUP_VIEWS[g]]);
    // No duplicates across (or within) groups.
    expect(new Set(assigned).size).toBe(assigned.length);
    // Complete coverage, both directions.
    expect([...assigned].sort()).toEqual([...ALL_VIEW_MODES].sort());
  });

  it("puts each group's home page inside that group", () => {
    for (const group of NAV_GROUPS) {
      expect(navGroupOf(NAV_GROUP_HOME[group])).toBe(group);
    }
  });

  it("routes the monitoring / management pages to 管家", () => {
    for (const view of ["gallery", "list", "audit", "report", "memory", "skills", "plugins", "mobile"] as const) {
      expect(navGroupOf(view)).toBe("steward");
    }
  });

  it("routes the agent-work pages to 工作", () => {
    for (const view of ["history", "files", "wiki", "schedule", "plans"] as const) {
      expect(navGroupOf(view)).toBe("work");
    }
  });

  it("rejects non-group values read back from storage", () => {
    expect(isNavGroup("steward")).toBe(true);
    expect(isNavGroup("work")).toBe(true);
    expect(isNavGroup("")).toBe(false);
    expect(isNavGroup(undefined)).toBe(false);
    expect(isNavGroup("gallery")).toBe(false);
  });
});
