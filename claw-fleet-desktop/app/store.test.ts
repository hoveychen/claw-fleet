import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionInfo } from "./types";

// navigateToSessionDetail → useDetailStore.open touches the tauri bridge; stub
// it so the store logic runs headless.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => {}),
}));

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
    updateMainViewState("audit", {
      tab: "rules",
      filter: "high",
      unreadOnly: false,
      category: "network",
      workspace: "/repo",
      selectedEventKey: "session:timestamp:Bash",
      selectedRuleId: "unsafe-shell",
      rulesQuery: "curl",
      allowRulesExpanded: true,
    });
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
    updateMainViewState("files", {
      selectedWorkspace: "/repo",
      activeRootPath: "/repo/.worktrees/feature",
      showIgnored: true,
      tab: "procs",
      activeFilePath: "src/main.rs",
    });
    updateMainViewState("mobile", { editingUrl: true, urlDraft: "wss://relay.example" });

    const state = useUIStore.getState().mainViewState;
    expect(state.gallery).toMatchObject({ query: "worker", showAll: true, idleExpanded: true });
    expect(state.audit).toEqual({
      tab: "rules",
      filter: "high",
      unreadOnly: false,
      category: "network",
      workspace: "/repo",
      selectedEventKey: "session:timestamp:Bash",
      selectedRuleId: "unsafe-shell",
      rulesQuery: "curl",
      allowRulesExpanded: true,
    });
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
    expect(state.files).toEqual({
      selectedWorkspace: "/repo",
      activeRootPath: "/repo/.worktrees/feature",
      showIgnored: true,
      tab: "procs",
      activeFilePath: "src/main.rs",
    });
    expect(state.mobile).toMatchObject({ editingUrl: true, urlDraft: "wss://relay.example" });
  });

  it("does not persist page browsing context across app restarts", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().updateMainViewState("wiki", { query: "temporary search" });

    expect(getItem("main-view-state")).toBeNull();
  });
});

/**
 * gallery is the default session layout, but two paths used to silently pin
 * users to `list` instead: (1) navigateToSessionDetail force-switched to list
 * on every notification/tray click, persisting it as the new default even
 * though the detail drawer renders under gallery too; (2) users who already
 * had `list` on disk from that bug never got flipped to the new default.
 */
describe("gallery 默认会话视图", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("在画廊视图下打开非 Fleet 会话时保持画廊,不强切列表", async () => {
    const { useUIStore, navigateToSessionDetail } = await import("./store");

    useUIStore.getState().setViewMode("gallery");
    expect(useUIStore.getState().viewMode).toBe("gallery");

    const nonFleet = {
      entrypoint: "cli",
      isSubagent: false,
      fleetSpawned: false,
      jsonlPath: "/tmp/x.jsonl",
    } as SessionInfo;

    // open() already lands on a session view on its own; the caller must not
    // clobber an existing gallery view with a hard `list`.
    navigateToSessionDetail(nonFleet);

    expect(useUIStore.getState().viewMode).toBe("gallery");
  });

  it("一次性把老用户已存的 list 翻成 gallery", async () => {
    const { setItem, getItem, migrateSessionViewDefault } = await import("./storage");
    setItem("viewMode", "list");
    setItem("lastSessionViewMode", "list");

    migrateSessionViewDefault();

    expect(getItem("viewMode")).toBe("gallery");
    expect(getItem("lastSessionViewMode")).toBe("gallery");
    expect(getItem("gallery-default-migrated")).toBe("true");
  });

  it("迁移过后尊重用户重新选择的 list", async () => {
    const { setItem, getItem, migrateSessionViewDefault } = await import("./storage");
    setItem("gallery-default-migrated", "true");
    setItem("viewMode", "list");

    migrateSessionViewDefault();

    expect(getItem("viewMode")).toBe("list");
  });
});
