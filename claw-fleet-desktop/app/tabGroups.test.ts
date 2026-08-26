import { describe, expect, it } from "vitest";
import {
  activeGroup,
  canFitAnotherGroup,
  closeGroup,
  closeTabAnywhere,
  FIRST_GROUP_ID,
  findGroupOf,
  focusGroup,
  focusGroupAt,
  inGroup,
  moveTabToGroup,
  nextGroupId,
  openTabInActiveGroup,
  parsePersistedGroups,
  pruneMissingGroupTabs,
  replaceTabAnywhere,
  resizeGroups,
  serializeGroups,
  singleGroup,
  splitGroup,
  openSecondView,
  openTabRouted,
  tabAffinity,
  visibleTabIds,
  type GroupsState,
  type TabGroup,
} from "./tabGroups";
import { closeTab, DRAFT_TAB_ID } from "./sessionTabs";
import { fileTabId, sessionViewTabId, webTabId, wikiTabId } from "./tabKind";

/** Terse group literal. `weight` defaults to an equal share, which doubles as
 *  regression cover: any reducer that rebuilds a group without carrying its
 *  weight across fails the existing `toEqual` assertions. */
const g = (
  id: string,
  tabIds: string[],
  activeId: string | null,
  weight = 1,
): TabGroup => ({ id, tabIds, activeId, weight });

/** Terse state literal, defaulting the axis to the side-by-side one. */
const st = (
  groups: TabGroup[],
  activeGroupId: string,
  orientation: "row" | "column" = "row",
): GroupsState => ({ groups, activeGroupId, orientation });

/**
 * Four invariants every reducer must preserve. They are asserted centrally
 * rather than case by case because each one, when broken, fails *silently*:
 * a dangling activeGroupId renders an empty column, and the same session id
 * living in two groups would mean two panes claiming one stateful session.
 */
function expectInvariants(s: GroupsState) {
  expect(s.groups.length).toBeGreaterThanOrEqual(1);
  expect(s.groups.map((x) => x.id)).toContain(s.activeGroupId);
  const seen = new Set<string>();
  for (const grp of s.groups) {
    for (const id of grp.tabIds) {
      expect(seen.has(id)).toBe(false);
      seen.add(id);
    }
    if (grp.activeId !== null) expect(grp.tabIds).toContain(grp.activeId);
  }
  expect(new Set(s.groups.map((x) => x.id)).size).toBe(s.groups.length);
}

describe("singleGroup", () => {
  it("wraps a flat tab state into the one-group layout", () => {
    expect(singleGroup({ tabIds: ["a", "b"], activeId: "b" })).toEqual(
      st([g(FIRST_GROUP_ID, ["a", "b"], "b")], FIRST_GROUP_ID),
    );
  });
});

describe("nextGroupId", () => {
  it("picks one past the highest existing number, not the count", () => {
    // Counting would collide after a middle group is reclaimed.
    expect(nextGroupId([g("g1", [], null), g("g7", [], null)])).toBe("g8");
  });

  it("ignores ids that are not in the g<N> form", () => {
    expect(nextGroupId([g("weird", [], null)])).toBe("g1");
  });
});

describe("inGroup", () => {
  it("routes a single-group reducer at the named group only", () => {
    const s = st([g("g1", ["a", "b"], "a"), g("g2", ["c"], "c")], "g1");
    const next = inGroup(s, "g2", (t) => closeTab(t, "c"));
    // g2 emptied, so it is reclaimed and focus lands on the survivor.
    expect(next).toEqual(st([g("g1", ["a", "b"], "a")], "g1"));
    expectInvariants(next);
  });

  it("keeps the last group alive even when it empties", () => {
    const s = st([g("g1", ["a"], "a")], "g1");
    const next = inGroup(s, "g1", (t) => closeTab(t, "a"));
    expect(next).toEqual(st([g("g1", [], null)], "g1"));
    expectInvariants(next);
  });

  it("moves focus off a reclaimed group that held it", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g2");
    const next = inGroup(s, "g2", (t) => closeTab(t, "b"));
    expect(next.activeGroupId).toBe("g1");
    expectInvariants(next);
  });

  it("is a no-op for an unknown group id", () => {
    const s = st([g("g1", ["a"], "a")], "g1");
    expect(inGroup(s, "nope", (t) => closeTab(t, "a"))).toBe(s);
  });
});

describe("splitGroup", () => {
  it("inserts an empty group right after the split one and focuses it", () => {
    // Empty on purpose: a tab is one *stateful session pane*, not a duplicable
    // file view, so VS Code's "same editor in both groups" cannot apply. The
    // new group waits for the user to fill it from the list.
    const s = st([g("g1", ["a", "b"], "a")], "g1");
    const next = splitGroup(s, "g1", "row");
    expect(next).toEqual(
      st([g("g1", ["a", "b"], "a"), g("g2", [], null)], "g2"),
    );
    expectInvariants(next);
  });

  it("inserts after the split group, not at the end", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g1");
    const next = splitGroup(s, "g1", "row");
    expect(next.groups.map((x) => x.id)).toEqual(["g1", "g3", "g2"]);
    expectInvariants(next);
  });

  it("adopts the requested axis for the whole column", () => {
    // The layout is flat, so orientation is global — splitting down flips it.
    const s = st([g("g1", ["a"], "a")], "g1", "row");
    expect(splitGroup(s, "g1", "column").orientation).toBe("column");
  });
});

describe("focusGroup / focusGroupAt", () => {
  it("moves focus to an existing group", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g1");
    expect(focusGroup(s, "g2").activeGroupId).toBe("g2");
  });

  it("ignores a focus request for a group that is gone", () => {
    const s = st([g("g1", ["a"], "a")], "g1");
    expect(focusGroup(s, "g9")).toBe(s);
  });

  it("focuses by 1-based position for the ⌘1..⌘9 bindings", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g1");
    expect(focusGroupAt(s, 2).activeGroupId).toBe("g2");
  });

  it("clamps a position past the last group to the last one", () => {
    // ⌘9 in VS Code lands on the last group rather than doing nothing.
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g1");
    expect(focusGroupAt(s, 9).activeGroupId).toBe("g2");
  });
});

describe("moveTabToGroup", () => {
  it("moves a tab across groups and focuses it in the destination", () => {
    const s = st([g("g1", ["a", "b"], "a"), g("g2", ["c"], "c")], "g1");
    const next = moveTabToGroup(s, "b", "g2");
    expect(next).toEqual(
      st([g("g1", ["a"], "a"), g("g2", ["c", "b"], "b")], "g2"),
    );
    expectInvariants(next);
  });

  it("re-anchors the source group's focus when the moved tab held it", () => {
    const s = st([g("g1", ["a", "b"], "b"), g("g2", [], null)], "g1");
    const next = moveTabToGroup(s, "b", "g2");
    expect(next.groups[0]).toEqual(g("g1", ["a"], "a"));
    expectInvariants(next);
  });

  it("reclaims the source group when the move empties it", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g1");
    const next = moveTabToGroup(s, "a", "g2");
    expect(next).toEqual(st([g("g2", ["b", "a"], "a")], "g2"));
    expectInvariants(next);
  });

  it("inserts before the named neighbour so a drop lands where it looks", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["x", "y"], "x")], "g1");
    const next = moveTabToGroup(s, "a", "g2", "y");
    expect(next.groups[0].tabIds).toEqual(["x", "a", "y"]);
    expectInvariants(next);
  });

  it("reorders within one group when source and destination match", () => {
    const s = st([g("g1", ["a", "b", "c"], "a")], "g1");
    const next = moveTabToGroup(s, "c", "g1", "a");
    expect(next.groups[0].tabIds).toEqual(["c", "a", "b"]);
    expectInvariants(next);
  });

  it("is a no-op for a tab nobody holds", () => {
    const s = st([g("g1", ["a"], "a")], "g1");
    expect(moveTabToGroup(s, "ghost", "g1")).toBe(s);
  });
});

describe("openTabInActiveGroup", () => {
  it("opens into the focused group", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", [], null)], "g2");
    const next = openTabInActiveGroup(s, "b");
    expect(next).toEqual(st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g2"));
    expectInvariants(next);
  });

  it("focuses the group that already holds the tab instead of duplicating it", () => {
    // Invariant 3: one stateful session pane cannot live in two groups.
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g2");
    const next = openTabInActiveGroup(s, "a");
    expect(next).toEqual(st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g1"));
    expectInvariants(next);
  });
});

describe("closeTabAnywhere / closeGroup", () => {
  it("closes a tab held by a background group", () => {
    const s = st([g("g1", ["a", "b"], "a"), g("g2", ["c"], "c")], "g1");
    const next = closeTabAnywhere(s, "b");
    expect(next).toEqual(st([g("g1", ["a"], "a"), g("g2", ["c"], "c")], "g1"));
    expectInvariants(next);
  });

  it("drops a whole group and everything in it", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["b", "c"], "b")], "g2");
    const next = closeGroup(s, "g2");
    expect(next).toEqual(st([g("g1", ["a"], "a")], "g1"));
    expectInvariants(next);
  });

  it("empties rather than removes the last group", () => {
    const s = st([g("g1", ["a"], "a")], "g1");
    const next = closeGroup(s, "g1");
    expect(next).toEqual(st([g("g1", [], null)], "g1"));
    expectInvariants(next);
  });
});

describe("replaceTabAnywhere", () => {
  it("morphs the draft tab in whichever group holds it", () => {
    const s = st(
      [g("g1", ["a"], "a"), g("g2", [DRAFT_TAB_ID], DRAFT_TAB_ID)],
      "g2",
    );
    const next = replaceTabAnywhere(s, DRAFT_TAB_ID, "real");
    expect(next).toEqual(st([g("g1", ["a"], "a"), g("g2", ["real"], "real")], "g2"));
    expectInvariants(next);
  });

  it("falls back to opening in the active group when the draft is gone", () => {
    const s = st([g("g1", ["a"], "a")], "g1");
    const next = replaceTabAnywhere(s, DRAFT_TAB_ID, "real");
    expect(next.groups[0].tabIds).toEqual(["a", "real"]);
    expectInvariants(next);
  });
});

describe("pruneMissingGroupTabs", () => {
  it("drops vanished sessions from every group and reclaims the emptied ones", () => {
    const s = st([g("g1", ["a", "gone"], "gone"), g("g2", ["gone2"], "gone2")], "g2");
    const next = pruneMissingGroupTabs(s, (id) => id === "a");
    expect(next).toEqual(st([g("g1", ["a"], "a")], "g1"));
    expectInvariants(next);
  });

  it("returns the same object when nothing is missing, so React can bail out", () => {
    const s = st([g("g1", ["a"], "a")], "g1");
    expect(pruneMissingGroupTabs(s, () => true)).toBe(s);
  });
});

describe("visibleTabIds", () => {
  it("reports one visible tab per group — the set that must stay unpaused", () => {
    const s = st([g("g1", ["a", "b"], "a"), g("g2", ["c"], "c")], "g1");
    expect(visibleTabIds(s)).toEqual(new Set(["a", "c"]));
  });
});

describe("parsePersistedGroups", () => {
  it("round-trips the serialized layout", () => {
    const s = st([g("g1", ["a"], "a"), g("g4", ["b"], "b")], "g4", "column");
    expect(parsePersistedGroups(serializeGroups(s))).toEqual(s);
  });

  it("migrates the pre-split flat blob into one group", () => {
    // Without this, every existing user loses their open tabs on upgrade.
    const raw = JSON.stringify({ tabIds: ["a", "b"], activeId: "b" });
    expect(parsePersistedGroups(raw)).toEqual(
      st([g(FIRST_GROUP_ID, ["a", "b"], "b")], FIRST_GROUP_ID),
    );
  });

  it("degrades a corrupt blob to one empty group rather than throwing", () => {
    expect(parsePersistedGroups("{not json")).toEqual(
      st([g(FIRST_GROUP_ID, [], null)], FIRST_GROUP_ID),
    );
    expect(parsePersistedGroups(null)).toEqual(
      st([g(FIRST_GROUP_ID, [], null)], FIRST_GROUP_ID),
    );
  });

  it("de-duplicates a tab that a half-written blob put in two groups", () => {
    const raw = JSON.stringify({
      groups: [
        { id: "g1", tabIds: ["a", "b"], activeId: "a" },
        { id: "g2", tabIds: ["b", "c"], activeId: "b" },
      ],
      activeGroupId: "g2",
      orientation: "row",
    });
    const next = parsePersistedGroups(raw);
    expect(next.groups.map((x) => x.tabIds)).toEqual([["a", "b"], ["c"]]);
    // g2's activeId pointed at the duplicate it just lost — re-anchored.
    expect(next.groups[1].activeId).toBe("c");
    expectInvariants(next);
  });

  it("re-anchors a dangling activeGroupId and a bad orientation", () => {
    const raw = JSON.stringify({
      groups: [{ id: "g1", tabIds: ["a"], activeId: "a" }],
      activeGroupId: "ghost",
      orientation: "diagonal",
    });
    const next = parsePersistedGroups(raw);
    expect(next.activeGroupId).toBe("g1");
    expect(next.orientation).toBe("row");
    expectInvariants(next);
  });

  it("drops groups that survive as empty shells, keeping one", () => {
    const raw = JSON.stringify({
      groups: [
        { id: "g1", tabIds: [], activeId: null },
        { id: "g2", tabIds: ["a"], activeId: "a" },
      ],
      activeGroupId: "g1",
      orientation: "row",
    });
    const next = parsePersistedGroups(raw);
    expect(next).toEqual(st([g("g2", ["a"], "a")], "g2"));
    expectInvariants(next);
  });
});

describe("resizeGroups", () => {
  // 1000px axis, two equal groups → 500px each. Weights are relative, so the
  // helper converts to px, clamps, and converts back.
  const two = [1, 1];

  it("moves the boundary by the dragged distance", () => {
    const next = resizeGroups(two, 0, 100, 1000, 200);
    // 600 / 400 of a 1000px axis, expressed against the same weight total (2).
    expect(next).toEqual([1.2, 0.8]);
  });

  it("keeps the total weight constant so untouched groups don't move", () => {
    const next = resizeGroups([1, 1, 2], 0, 150, 1000, 200);
    expect(next[2]).toBe(2);
    expect(next.reduce((a, b) => a + b, 0)).toBeCloseTo(4);
  });

  it("clamps at the minimum instead of collapsing a group", () => {
    // Dragging 400px left would leave the first group at 100px.
    const next = resizeGroups(two, 0, -400, 1000, 200);
    expect(next[0]).toBeCloseTo(0.4); // 200px of 1000, total weight 2
    expect(next[1]).toBeCloseTo(1.6);
  });

  it("clamps the far side symmetrically", () => {
    const next = resizeGroups(two, 0, 400, 1000, 200);
    expect(next[1]).toBeCloseTo(0.4);
  });

  it("refuses when the axis cannot even hold two minimums", () => {
    // No arrangement satisfies the constraint, so leave the layout alone rather
    // than picking an arbitrary violation.
    expect(resizeGroups(two, 0, 50, 300, 200)).toEqual(two);
  });

  it("is a no-op for a boundary that doesn't exist", () => {
    expect(resizeGroups(two, 1, 50, 1000, 200)).toEqual(two);
    expect(resizeGroups(two, -1, 50, 1000, 200)).toEqual(two);
  });

  it("is a no-op for a zero drag", () => {
    expect(resizeGroups(two, 0, 0, 1000, 200)).toEqual(two);
  });
});

describe("canFitAnotherGroup", () => {
  it("allows a split while every resulting group clears the minimum", () => {
    // 900px / 3 groups = 300 each ≥ 280.
    expect(canFitAnotherGroup(900, 2, 280)).toBe(true);
  });

  it("refuses a split that would push groups under the minimum", () => {
    // 800 / 3 = 266 < 280.
    expect(canFitAnotherGroup(800, 2, 280)).toBe(false);
  });

  it("allows the first split of a wide column", () => {
    expect(canFitAnotherGroup(1000, 1, 280)).toBe(true);
  });

  it("allows it while the axis is still unmeasured", () => {
    // 0 means "ResizeObserver hasn't reported yet". Blocking then would make
    // the split buttons dead on first paint.
    expect(canFitAnotherGroup(0, 1, 280)).toBe(true);
  });
});

describe("activeGroup / findGroupOf", () => {
  it("resolves the focused group", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g2");
    expect(activeGroup(s).id).toBe("g2");
  });

  it("finds which group holds a tab", () => {
    const s = st([g("g1", ["a"], "a"), g("g2", ["b"], "b")], "g1");
    expect(findGroupOf(s, "b")?.id).toBe("g2");
    expect(findGroupOf(s, "ghost")).toBe(null);
  });
});

describe("tabAffinity", () => {
  it("counts sessions and the new-session draft as primary", () => {
    expect(tabAffinity("7c9e6679-7425-40de-944b-e07fc1f90ae7")).toBe("primary");
    expect(tabAffinity(DRAFT_TAB_ID)).toBe("primary");
  });

  it("counts a second view of a session as primary too", () => {
    // It is a session pane, so a path clicked inside it must route to the docs
    // half rather than landing on top of the transcript it was clicked in.
    expect(tabAffinity(sessionViewTabId("7c9e6679-7425-40de-944b-e07fc1f90ae7"))).toBe(
      "primary",
    );
  });

  it("counts the reading material as side", () => {
    expect(tabAffinity(fileTabId("/repo/main.rs"))).toBe("side");
    expect(tabAffinity(wikiTabId("arch/overview"))).toBe("side");
    expect(tabAffinity(webTabId("https://example.com"))).toBe("side");
  });
});

/**
 * The routing heuristic: conversations collect in one group, the material they
 * cite in another, without the user arranging it by hand every time.
 */
describe("openTabRouted", () => {
  const SESSION = "sess-a";
  const OTHER_SESSION = "sess-b";
  const FILE = fileTabId("/repo/main.rs");
  const FILE2 = fileTabId("/repo/lib.rs");
  const WIKI = wikiTabId("arch/overview");

  it("opens the first side tab in a group of its own, splitting to make room", () => {
    const s = st([g("g1", [SESSION], SESSION)], "g1");
    const out = openTabRouted(s, FILE, true);

    expect(out.groups.map((x) => x.tabIds)).toEqual([[SESSION], [FILE]]);
    // Focus follows what you just opened, as clicking a tab does.
    expect(out.activeGroupId).toBe(out.groups[1].id);
    expectInvariants(out);
  });

  it("puts the next side tab in the side group that now exists", () => {
    const s = st([g("g1", [SESSION], SESSION), g("g2", [FILE], FILE)], "g1");
    const out = openTabRouted(s, WIKI, true);

    expect(out.groups.length).toBe(2);
    expect(out.groups[1].tabIds).toEqual([FILE, WIKI]);
    expect(out.activeGroupId).toBe("g2");
    expectInvariants(out);
  });

  it("sends a session back to the group holding the conversations", () => {
    // Focus is parked in the side group (you were reading a doc) and you click a
    // session row: it belongs with the other conversation, not among the docs.
    const s = st([g("g1", [SESSION], SESSION), g("g2", [FILE], FILE)], "g2");
    const out = openTabRouted(s, OTHER_SESSION, true);

    expect(out.groups.length).toBe(2);
    expect(out.groups[0].tabIds).toEqual([SESSION, OTHER_SESSION]);
    expect(out.activeGroupId).toBe("g1");
    expectInvariants(out);
  });

  it("keeps a tab that is already open where it is, and reveals it", () => {
    const s = st([g("g1", [SESSION], SESSION), g("g2", [FILE, WIKI], WIKI)], "g1");
    const out = openTabRouted(s, FILE, true);

    expect(out.groups.map((x) => x.tabIds)).toEqual([[SESSION], [FILE, WIKI]]);
    expect(out.groups[1].activeId).toBe(FILE);
    expect(out.activeGroupId).toBe("g2");
    expectInvariants(out);
  });

  it("fills an empty group the user just split open, whatever the kind", () => {
    // They made room on purpose; routing past it would leave a blank half.
    const s = st([g("g1", [SESSION], SESSION), g("g2", [], null)], "g2");
    const out = openTabRouted(s, FILE, true);

    expect(out.groups.map((x) => x.tabIds)).toEqual([[SESSION], [FILE]]);
    expect(out.activeGroupId).toBe("g2");
    expectInvariants(out);
  });

  it("stays in the focused group when the column is too narrow to split", () => {
    // The width gate wins: a split that produced unreadable slivers is worse
    // than a mixed group.
    const s = st([g("g1", [SESSION], SESSION)], "g1");
    const out = openTabRouted(s, FILE, false);

    expect(out.groups.length).toBe(1);
    expect(out.groups[0].tabIds).toEqual([SESSION, FILE]);
    expectInvariants(out);
  });

  it("respects a group the user mixed by hand instead of splitting again", () => {
    // A file dragged in beside a session makes that group a home for both, so
    // the heuristic stops fighting the arrangement.
    const s = st([g("g1", [SESSION, FILE], SESSION)], "g1");
    const out = openTabRouted(s, FILE2, true);

    expect(out.groups.length).toBe(1);
    expect(out.groups[0].tabIds).toEqual([SESSION, FILE, FILE2]);
    expectInvariants(out);
  });

  it("picks the nearest matching group, preferring the one to the right", () => {
    const s = st(
      [g("g1", [FILE], FILE), g("g2", [SESSION], SESSION), g("g3", [FILE2], FILE2)],
      "g2",
    );
    const out = openTabRouted(s, WIKI, true);

    expect(out.groups.length).toBe(3);
    expect(out.groups[2].tabIds).toEqual([FILE2, WIKI]);
    expect(out.activeGroupId).toBe("g3");
    expectInvariants(out);
  });

  it("splits along the column's current axis, not always sideways", () => {
    const s = st([g("g1", [SESSION], SESSION)], "g1", "column");
    const out = openTabRouted(s, FILE, true);

    expect(out.orientation).toBe("column");
    expectInvariants(out);
  });
});

/**
 * The second view: one session, two panes, so a conversation and the artefacts
 * it produced can sit side by side.
 */
describe("openSecondView", () => {
  const SESSION = "sess-a";
  const VIEW = sessionViewTabId(SESSION);
  const FILE = fileTabId("/repo/main.rs");

  it("splits a lone group and puts the copy in the new half", () => {
    const s = st([g("g1", [SESSION], SESSION)], "g1");
    const out = openSecondView(s, SESSION, true);

    expect(out.groups.map((x) => x.tabIds)).toEqual([[SESSION], [VIEW]]);
    // Focus follows what you just opened — you asked to look at it.
    expect(out.activeGroupId).toBe(out.groups[1].id);
    expectInvariants(out);
  });

  it("does NOT land the copy in the conversation's own group", () => {
    // The whole point. Routing by kind would do exactly this, since both views
    // are the same kind — which is why this has its own reducer.
    const s = st([g("g1", [SESSION], SESSION)], "g1");
    const out = openSecondView(s, SESSION, true);
    expect(out.groups[0].tabIds).toEqual([SESSION]);
  });

  it("reuses the neighbouring group rather than splitting again", () => {
    const s = st([g("g1", [SESSION], SESSION), g("g2", [FILE], FILE)], "g1");
    const out = openSecondView(s, SESSION, true);

    expect(out.groups).toHaveLength(2);
    expect(out.groups[1].tabIds).toEqual([FILE, VIEW]);
    expect(out.groups[1].activeId).toBe(VIEW);
    expect(out.activeGroupId).toBe("g2");
    expectInvariants(out);
  });

  it("reveals the copy instead of opening a third pane", () => {
    const s = st([g("g1", [SESSION], SESSION), g("g2", [VIEW, FILE], FILE)], "g1");
    const out = openSecondView(s, SESSION, true);

    expect(out.groups.map((x) => x.tabIds)).toEqual([[SESSION], [VIEW, FILE]]);
    expect(out.groups[1].activeId).toBe(VIEW);
    expect(out.activeGroupId).toBe("g2");
    expectInvariants(out);
  });

  it("falls back to the conversation's group when the column can't split", () => {
    // Never a silent no-op: it lands as a tab you can drag out once there's room.
    const s = st([g("g1", [SESSION], SESSION)], "g1");
    const out = openSecondView(s, SESSION, false);

    expect(out.groups).toHaveLength(1);
    expect(out.groups[0].tabIds).toEqual([SESSION, VIEW]);
    expect(out.groups[0].activeId).toBe(VIEW);
    expectInvariants(out);
  });

  it("splits from the focused group when the session isn't open at all", () => {
    const s = st([g("g1", [FILE], FILE)], "g1");
    const out = openSecondView(s, SESSION, true);

    expect(out.groups.map((x) => x.tabIds)).toEqual([[FILE], [VIEW]]);
    expectInvariants(out);
  });

  it("keeps the two views independent: closing one leaves the other", () => {
    const s = st([g("g1", [SESSION], SESSION)], "g1");
    const split = openSecondView(s, SESSION, true);
    const out = closeTabAnywhere(split, VIEW);

    expect(out.groups.map((x) => x.tabIds)).toEqual([[SESSION]]);
    expectInvariants(out);
  });
});
