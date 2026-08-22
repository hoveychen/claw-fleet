import { describe, expect, it } from "vitest";
import {
  activeGroup,
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
  serializeGroups,
  singleGroup,
  splitGroup,
  visibleTabIds,
  type GroupsState,
  type TabGroup,
} from "./tabGroups";
import { closeTab, DRAFT_TAB_ID } from "./sessionTabs";

/** Terse group literal. */
const g = (id: string, tabIds: string[], activeId: string | null): TabGroup => ({
  id,
  tabIds,
  activeId,
});

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
