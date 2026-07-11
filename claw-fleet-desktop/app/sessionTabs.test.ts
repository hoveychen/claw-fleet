import { describe, expect, it } from "vitest";
import {
  closeOtherTabs,
  closeTab,
  closeTabsToRight,
  openTab,
  parsePersistedTabs,
  pruneMissingTabs,
  reorderTabs,
  type TabState,
} from "./sessionTabs";

const state = (tabIds: string[], activeId: string | null): TabState => ({
  tabIds,
  activeId,
});

describe("openTab", () => {
  it("appends a new tab to the right and focuses it", () => {
    expect(openTab(state(["a", "b"], "a"), "c")).toEqual(
      state(["a", "b", "c"], "c"),
    );
  });

  it("focuses an already-open tab instead of duplicating it", () => {
    expect(openTab(state(["a", "b"], "b"), "a")).toEqual(
      state(["a", "b"], "a"),
    );
  });
});

describe("closeTab", () => {
  it("moves focus to the right neighbour when closing the focused tab", () => {
    expect(closeTab(state(["a", "b", "c"], "b"), "b")).toEqual(
      state(["a", "c"], "c"),
    );
  });

  it("falls back to the left neighbour when closing the rightmost tab", () => {
    expect(closeTab(state(["a", "b", "c"], "c"), "c")).toEqual(
      state(["a", "b"], "b"),
    );
  });

  it("leaves focus alone when closing a background tab", () => {
    expect(closeTab(state(["a", "b", "c"], "a"), "c")).toEqual(
      state(["a", "b"], "a"),
    );
  });

  it("clears focus when the last tab is closed", () => {
    expect(closeTab(state(["a"], "a"), "a")).toEqual(state([], null));
  });

  it("is a no-op for a tab that isn't open", () => {
    const before = state(["a", "b"], "a");
    expect(closeTab(before, "zz")).toBe(before);
  });
});

describe("closeOtherTabs", () => {
  it("keeps only the named tab and focuses it, even from a background tab", () => {
    expect(closeOtherTabs(state(["a", "b", "c"], "a"), "c")).toEqual(
      state(["c"], "c"),
    );
  });
});

describe("closeTabsToRight", () => {
  it("drops everything right of the named tab", () => {
    expect(closeTabsToRight(state(["a", "b", "c", "d"], "a"), "b")).toEqual(
      state(["a", "b"], "a"),
    );
  });

  it("rehomes focus onto the named tab when the focused one got closed", () => {
    expect(closeTabsToRight(state(["a", "b", "c", "d"], "d"), "b")).toEqual(
      state(["a", "b"], "b"),
    );
  });
});

describe("parsePersistedTabs", () => {
  it("round-trips a strip written by a previous run", () => {
    const raw = JSON.stringify(state(["a", "b"], "b"));
    expect(parsePersistedTabs(raw)).toEqual(state(["a", "b"], "b"));
  });

  it("degrades to an empty strip instead of throwing on junk", () => {
    // A corrupt payload must not be able to take the 启动台 down with it.
    expect(parsePersistedTabs(null)).toEqual(state([], null));
    expect(parsePersistedTabs("not json{{")).toEqual(state([], null));
    expect(parsePersistedTabs("null")).toEqual(state([], null));
    expect(parsePersistedTabs('"a string"')).toEqual(state([], null));
    expect(parsePersistedTabs("{}")).toEqual(state([], null));
    expect(parsePersistedTabs('{"tabIds":"nope"}')).toEqual(state([], null));
  });

  it("drops non-string ids", () => {
    expect(parsePersistedTabs('{"tabIds":["a",7,null,"b"],"activeId":"b"}')).toEqual(
      state(["a", "b"], "b"),
    );
  });

  it("re-anchors focus when the persisted activeId isn't in the list", () => {
    expect(parsePersistedTabs('{"tabIds":["a","b"],"activeId":"gone"}')).toEqual(
      state(["a", "b"], "a"),
    );
    expect(parsePersistedTabs('{"tabIds":["a","b"]}')).toEqual(
      state(["a", "b"], "a"),
    );
  });
});

describe("pruneMissingTabs", () => {
  const alive = (ids: string[]) => (id: string) => ids.includes(id);

  it("drops tabs whose session is gone from the scan", () => {
    expect(pruneMissingTabs(state(["a", "b", "c"], "a"), alive(["a", "c"]))).toEqual(
      state(["a", "c"], "a"),
    );
  });

  it("re-anchors focus when the focused session is the one that vanished", () => {
    expect(pruneMissingTabs(state(["a", "b"], "b"), alive(["a"]))).toEqual(
      state(["a"], "a"),
    );
  });

  it("clears focus when every session is gone", () => {
    expect(pruneMissingTabs(state(["a", "b"], "a"), alive([]))).toEqual(
      state([], null),
    );
  });

  it("returns the same object when nothing is missing", () => {
    const before = state(["a", "b"], "a");
    expect(pruneMissingTabs(before, alive(["a", "b"]))).toBe(before);
  });
});

describe("reorderTabs", () => {
  it("moves a tab rightwards into the target's slot", () => {
    expect(reorderTabs(["a", "b", "c"], "a", "c")).toEqual(["b", "c", "a"]);
  });

  it("moves a tab leftwards into the target's slot", () => {
    expect(reorderTabs(["a", "b", "c"], "c", "a")).toEqual(["c", "a", "b"]);
  });

  it("is a no-op when the ids are unknown or identical", () => {
    const before = ["a", "b"];
    expect(reorderTabs(before, "a", "a")).toBe(before);
    expect(reorderTabs(before, "zz", "a")).toBe(before);
  });
});
