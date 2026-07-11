import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * 启动台's rail filters used to be component-local `useState` inside HistoryView.
 * SessionList mounts HistoryView through a `viewMode` ternary, so any hop to
 * another view — including the involuntary `setViewMode("list")` a waiting-input
 * alert or the mascot bubble performs — unmounted it and snapped the segmented
 * filter back to 「全部」. They live in useUIStore now, and the durable ones are
 * written through to the settings store so they also survive a restart.
 *
 * There is no jsdom/testing-library here, so the unmount itself isn't
 * exercisable; what these cover is the store contract that makes an unmount
 * harmless — the value lives outside React, and it round-trips through storage.
 */
describe("启动台 rail filters", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("writes the mark filter through to the settings store", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().setHistoryMarkFilter("done");

    expect(useUIStore.getState().historyMarkFilter).toBe("done");
    expect(getItem("history-mark-filter")).toBe("done");
  });

  it("restores a persisted mark filter on boot", async () => {
    const { setItem } = await import("./storage");
    setItem("history-mark-filter", "pending");

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().historyMarkFilter).toBe("pending");
  });

  it("falls back to 全部 when the persisted mark filter is corrupt", async () => {
    const { setItem } = await import("./storage");
    setItem("history-mark-filter", "not-a-segment");

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().historyMarkFilter).toBe("all");
  });

  it("round-trips the workspace filter and the active-only toggle", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().setHistoryWorkspaceFilter("/Users/x/workspace/maliang");
    useUIStore.getState().setHistoryActiveOnly(true);

    expect(getItem("history-workspace-filter")).toBe("/Users/x/workspace/maliang");
    expect(getItem("history-active-only")).toBe("true");
  });

  it("keeps the search box out of the settings store", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().setHistoryQuery("scene-items");

    // Survives a view switch (it's in the store) but must not come back on the
    // next launch — a restored query would fire an FTS search nobody asked for.
    expect(useUIStore.getState().historyQuery).toBe("scene-items");
    expect(getItem("history-query")).toBeNull();
  });
});
