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

describe("主导航页面浏览上下文", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("keeps all page navigation state outside component lifetimes", async () => {
    const { useUIStore } = await import("./store");
    const { updateMainViewState } = useUIStore.getState();

    updateMainViewState("gallery", { query: "worker", showAll: true, idleExpanded: true });
    updateMainViewState("audit", { tab: "rules", selectedRuleId: "unsafe-shell" });
    updateMainViewState("memory", {
      query: "lesson",
      filterType: "feedback",
      sourceFilter: "codex",
      workspaceFilter: "repo",
      expandedKeys: ["repo"],
      selectedKey: "file:repo:MEMORY.md",
      detailTab: "history",
    });
    updateMainViewState("wiki", {
      query: "transport",
      workspaceFilter: "/repo",
      sortKey: "title",
      selectedFolder: "arch",
      selectedSlug: "arch/overview",
      collapsedFolders: ["reference"],
      versionBySlug: { "arch/overview": "v2" },
    });
    updateMainViewState("skills", {
      query: "review",
      sourceFilter: "claude-code",
      selectedPath: "/skills/review",
      activeFilePath: "references/checklist.md",
      collapsedPaths: ["references"],
      fileQuery: "checklist",
    });
    updateMainViewState("plugins", {
      query: "github",
      selectedPluginId: "github@official",
      expanded: { enabled: true, downloaded: false, catalog: true },
    });
    updateMainViewState("files", { selectedWorkspace: "/repo", activeFilePath: "src/main.rs" });
    updateMainViewState("mobile", { editingUrl: true, urlDraft: "wss://relay.example" });

    const state = useUIStore.getState().mainViewState;
    expect(state.gallery).toMatchObject({ query: "worker", showAll: true, idleExpanded: true });
    expect(state.audit).toMatchObject({ tab: "rules", selectedRuleId: "unsafe-shell" });
    expect(state.memory).toEqual({
      query: "lesson",
      filterType: "feedback",
      sourceFilter: "codex",
      workspaceFilter: "repo",
      expandedKeys: ["repo"],
      selectedKey: "file:repo:MEMORY.md",
      detailTab: "history",
    });
    expect(state.wiki).toEqual({
      query: "transport",
      workspaceFilter: "/repo",
      sortKey: "title",
      selectedFolder: "arch",
      selectedSlug: "arch/overview",
      collapsedFolders: ["reference"],
      versionBySlug: { "arch/overview": "v2" },
    });
    expect(state.skills).toEqual({
      query: "review",
      sourceFilter: "claude-code",
      selectedPath: "/skills/review",
      activeFilePath: "references/checklist.md",
      collapsedPaths: ["references"],
      fileQuery: "checklist",
    });
    expect(state.plugins).toEqual({
      query: "github",
      selectedPluginId: "github@official",
      expanded: { enabled: true, downloaded: false, catalog: true },
    });
    expect(state.files).toMatchObject({ selectedWorkspace: "/repo", activeFilePath: "src/main.rs" });
    expect(state.mobile).toMatchObject({ editingUrl: true, urlDraft: "wss://relay.example" });
  });

  it("does not persist page browsing context across app restarts", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().updateMainViewState("wiki", { query: "temporary search" });

    expect(getItem("main-view-state")).toBeNull();
  });
});
