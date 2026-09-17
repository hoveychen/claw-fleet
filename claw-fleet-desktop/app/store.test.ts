import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionInfo } from "./types";

// applyWindowTheme talks to the tauri window API; capture what it hands over.
const winMock = vi.hoisted(() => ({
  setTheme: vi.fn(async (_value: unknown) => undefined),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => winMock }));

// navigateToSessionDetail → useDetailStore.open touches the tauri bridge; stub
// it so the store logic runs headless.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => {}),
}));

/**
 * The launchpad's rail filters used to be component-local `useState` inside HistoryView.
 * SessionList mounts HistoryView through a `viewMode` ternary, so any hop to
 * another view — including the involuntary `setViewMode("list")` a waiting-input
 * alert or the mascot bubble performs — unmounted it and snapped the segmented
 * filter back to "all". They live in useUIStore now, and the durable ones are
 * written through to the settings store so they also survive a restart.
 *
 * There is no jsdom/testing-library here, so the unmount itself isn't
 * exercisable; what these cover is the store contract that makes an unmount
 * harmless — the value lives outside React, and it round-trips through storage.
 */
describe("launchpad rail filters", () => {
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

  it("round-trips the workspace filter", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().setHistoryWorkspaceFilter("/Users/x/workspace/maliang");

    expect(getItem("history-workspace-filter")).toBe("/Users/x/workspace/maliang");
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

  /**
   * Workspace collapse state used to be `useState` in WorkspaceRailSection. Grouping is
   * computed from *filtered* rows, so when switching to "in-progress", any workspace
   * with no sessions in that bucket unmounts entirely and re-mounts with the expand state
   * reset — a collapsed fold is now blank.
   */
  it("toggles a workspace fold and writes it through", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().toggleHistoryWorkspaceCollapsed("/Users/x/w/chat");

    expect(useUIStore.getState().historyCollapsedWorkspaces).toEqual([
      "/Users/x/w/chat",
    ]);
    expect(getItem("history-collapsed-workspaces")).toBe(
      JSON.stringify(["/Users/x/w/chat"]),
    );

    // Toggling again unfolds it rather than piling up duplicates.
    useUIStore.getState().toggleHistoryWorkspaceCollapsed("/Users/x/w/chat");
    expect(useUIStore.getState().historyCollapsedWorkspaces).toEqual([]);
    expect(getItem("history-collapsed-workspaces")).toBe("[]");
  });

  it("restores persisted workspace folds on boot", async () => {
    const { setItem } = await import("./storage");
    setItem("history-collapsed-workspaces", JSON.stringify(["/a", "/b"]));

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().historyCollapsedWorkspaces).toEqual(["/a", "/b"]);
  });

  it("ignores a corrupt persisted fold list", async () => {
    const { setItem } = await import("./storage");
    setItem("history-collapsed-workspaces", "{not json");

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().historyCollapsedWorkspaces).toEqual([]);
  });
});

describe("primary nav page browsing context", () => {
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
 * The nav sidebar's Fleet / Work tabs. The active tab is *derived* from `viewMode`
 * (navGroupOf) rather than stored beside it, so the invariant worth testing is
 * the other half: each tab remembers the page you left it on, and every path
 * that moves the main area — the nav itself and the three cross-page requests
 * (file link, tray click, schedule → new session) — feeds that memory. A path
 * that wrote `viewMode` directly would leave its tab restoring a stale page.
 */
describe("nav sidebar mode tab (Fleet / Work)", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("switching tabs restores that tab's previously left page", async () => {
    const { useUIStore } = await import("./store");
    const ui = () => useUIStore.getState();

    ui().setViewMode("audit"); // Fleet
    ui().setViewMode("wiki"); // Work

    ui().setNavGroup("fleet");
    expect(ui().viewMode).toBe("audit");

    ui().setNavGroup("work");
    expect(ui().viewMode).toBe("wiki");
  });

  it("first entry into a tab lands on its home page", async () => {
    const { useUIStore } = await import("./store");

    useUIStore.getState().setNavGroup("work");
    expect(useUIStore.getState().viewMode).toBe("history");
  });

  it("clicking the current tab does not change pages", async () => {
    const { useUIStore } = await import("./store");

    useUIStore.getState().setViewMode("plans");
    useUIStore.getState().setNavGroup("work");

    expect(useUIStore.getState().viewMode).toBe("plans");
  });

  it("cross-page jumps that bypass nav also feed tab memory", async () => {
    const { useUIStore } = await import("./store");
    const ui = () => useUIStore.getState();

    // First set Work tab's memory to plan tree, so "home fallback" and "actually recorded"
    // results don't collapse into the same value — otherwise the assertion to jump to
    // history would pass even if not recorded.
    ui().setViewMode("plans");

    // Tray / notification click → task page (Work tab), goes via requestOpenTask not setViewMode.
    ui().requestOpenTask("sess-1");
    expect(ui().viewMode).toBe("history");

    ui().setNavGroup("fleet");
    expect(ui().viewMode).toBe("gallery");

    // If requestOpenTask wasn't recorded, we'd revert to the previous page (plans).
    ui().setNavGroup("work");
    expect(ui().viewMode).toBe("history");

    // File link click (requestFileNav) is the same kind of nav-bypassing path.
    ui().requestFileNav({ workspacePath: "/w", absPath: "/w/a.ts", line: 3 });
    ui().setNavGroup("fleet");
    ui().setNavGroup("work");
    expect(ui().viewMode).toBe("files");
  });

  it("writes each tab's last page to the settings store", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().setViewMode("memory");
    useUIStore.getState().setViewMode("files");

    expect(JSON.parse(getItem("nav-group-last-view")!)).toMatchObject({
      fleet: "memory",
      work: "files",
    });
  });

  it("discards stale pages no longer belonging to a tab and falls back to home", async () => {
    const { setItem } = await import("./storage");
    // wiki now belongs to Work tab; old data recording it under Fleet shouldn't let Fleet open it.
    setItem("nav-group-last-view", JSON.stringify({ fleet: "wiki", work: "nonsense" }));

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().lastViewByNavGroup).toEqual({
      fleet: "gallery",
      work: "history",
    });
  });

  it("recognizes steward memory written before the rename", async () => {
    const { setItem } = await import("./storage");
    // This tab's code was renamed from steward to fleet in 2026-08. On machines already
    // in use, the old key is still on disk; if we don't recognize it, the user's
    // "Fleet tab was last on memory page" state is silently lost.
    setItem("nav-group-last-view", JSON.stringify({ steward: "memory", work: "files" }));

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().lastViewByNavGroup).toEqual({
      fleet: "memory",
      work: "files",
    });
  });

  it("new key takes priority over simultaneously existing old key", async () => {
    const { setItem } = await import("./storage");
    setItem("nav-group-last-view", JSON.stringify({ fleet: "skills", steward: "memory", work: "files" }));

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().lastViewByNavGroup.fleet).toBe("skills");
  });
});

/**
 * gallery is the default session layout, but two paths used to silently pin
 * users to `list` instead: (1) navigateToSessionDetail force-switched to list
 * on every notification/tray click, persisting it as the new default even
 * though the detail drawer renders under gallery too; (2) users who already
 * had `list` on disk from that bug never got flipped to the new default.
 */
describe("gallery default session view", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("keeps gallery view when opening non-Fleet sessions in gallery, does not force switch to list", async () => {
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

  it("one-time migration of existing list view to gallery for old users", async () => {
    const { setItem, getItem, migrateSessionViewDefault } = await import("./storage");
    setItem("viewMode", "list");
    setItem("lastSessionViewMode", "list");

    migrateSessionViewDefault();

    expect(getItem("viewMode")).toBe("gallery");
    expect(getItem("lastSessionViewMode")).toBe("gallery");
    expect(getItem("gallery-default-migrated")).toBe("true");
  });

  it("after migration, respects user's new list choice", async () => {
    const { setItem, getItem, migrateSessionViewDefault } = await import("./storage");
    setItem("gallery-default-migrated", "true");
    setItem("viewMode", "list");

    migrateSessionViewDefault();

    expect(getItem("viewMode")).toBe("list");
  });
});

/**
 * The "Messages" tab gets stuck forever on "loading…".
 *
 * Controlled repro (2026-08-17, P1 harness + `kill -STOP` on dsh web): once the
 * backend stops responding, `get_messages_tail` never settles, and the detail pane
 * has **no deadline on this fetch** — title / token / four tabs render normally,
 * but the message area stays "loading…" for 80s without moving, single /messages
 * call unreturned for 120s. This is exactly what the user sees.
 *
 * Contract: if we haven't received a transcript by the deadline, we must stop
 * spinning and expose the state (stalled) so the UI can explain and offer retry;
 * late-arriving results still render, we can't discard them just because marked stalled.
 */
describe("detail fetch deadline", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useRealTimers();
  });

  const session = {
    id: "session-dsh-1",
    jsonlPath: "dsh://session-dsh-1",
    entrypoint: null,
    isSubagent: false,
    fleetSpawned: true,
  } as unknown as SessionInfo;

  it("when backend never returns, stops spinning at deadline and marks stalled", async () => {
    vi.useFakeTimers();
    const { invoke } = await import("@tauri-apps/api/core");
    (invoke as unknown as { mockImplementation: (f: unknown) => void }).mockImplementation(
      (cmd: string) =>
        cmd === "get_messages_tail" ? new Promise(() => {}) : Promise.resolve(undefined),
    );

    const { useDetailStore, TAIL_LOAD_DEADLINE_MS } = await import("./store");
    void useDetailStore.getState().open(session);
    await vi.advanceTimersByTimeAsync(1);
    expect(useDetailStore.getState().isLoading).toBe(true);

    await vi.advanceTimersByTimeAsync(TAIL_LOAD_DEADLINE_MS + 100);

    expect(useDetailStore.getState().isLoading).toBe(false);
    expect(useDetailStore.getState().loadStalled).toBe(true);
  });

  it("reopening clears the previous loadError", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mock = invoke as unknown as { mockImplementation: (f: unknown) => void };
    mock.mockImplementation((cmd: string) =>
      cmd === "get_messages_tail" ? Promise.reject(new Error("boom")) : Promise.resolve(undefined),
    );
    const { useDetailStore } = await import("./store");
    await useDetailStore.getState().open(session);
    expect(useDetailStore.getState().loadError).toContain("boom");

    mock.mockImplementation((cmd: string) =>
      cmd === "get_messages_tail" ? Promise.resolve([{ type: "user" }]) : Promise.resolve(undefined),
    );
    await useDetailStore.getState().open(session);
    expect(useDetailStore.getState().loadError).toBeNull();
  });

  it("late-arriving transcript still renders and clears stalled", async () => {
    vi.useFakeTimers();
    let land: (v: unknown) => void = () => {};
    const { invoke } = await import("@tauri-apps/api/core");
    (invoke as unknown as { mockImplementation: (f: unknown) => void }).mockImplementation(
      (cmd: string) =>
        cmd === "get_messages_tail"
          ? new Promise((res) => {
              land = res;
            })
          : Promise.resolve(undefined),
    );

    const { useDetailStore, TAIL_LOAD_DEADLINE_MS } = await import("./store");
    void useDetailStore.getState().open(session);
    await vi.advanceTimersByTimeAsync(TAIL_LOAD_DEADLINE_MS + 100);
    expect(useDetailStore.getState().loadStalled).toBe(true);

    land([{ type: "user" }]);
    await vi.advanceTimersByTimeAsync(10);

    expect(useDetailStore.getState().loadStalled).toBe(false);
    expect(useDetailStore.getState().messages).toHaveLength(1);
  });

  /**
   * The second entry point in the same scenario, the one the deadline doesn't cover.
   *
   * Inside `open()`, `isLoading` must wait for two awaits: `get_messages_tail` (with
   * 20s deadline protection) followed by `start_watching_session`. The latter acquires
   * the backend's **write lock**, while `get_messages_tail` / `read_live_thinking` take
   * read locks — on active sessions those two polls fire at 1.5s / 700ms intervals, so
   * the wait time for the write lock is determined by others' read locks, not by
   * transcript fetch time.
   *
   * But `withStallWatch` only wraps the first await: once transcript arrives, the deadline
   * is disarmed. So stalling on the second await means **permanent "loading…" with no retry
   * button** — the messages are actually in hand, just not set into the store.
   *
   * Contract: watcher registration is not on the critical render path. Once we have the
   * transcript, we render; registration failure or lateness must not hold back messages
   * we already have.
   */
  it("when watcher registration never returns, already-fetched transcript still renders", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    (invoke as unknown as { mockImplementation: (f: unknown) => void }).mockImplementation(
      (cmd: string) => {
        if (cmd === "get_messages_tail") return Promise.resolve([{ type: "user" }]);
        // Write lock never acquired — command sent out, never settles.
        if (cmd === "start_watching_session") return new Promise(() => {});
        return Promise.resolve(undefined);
      },
    );

    const { useDetailStore } = await import("./store");
    // Don't await open(): it's already stuck on that never-settling registration, and await
    // would turn this test into a timeout rather than a clean assertion failure.
    void useDetailStore.getState().open(session);
    for (let i = 0; i < 20; i++) await Promise.resolve();

    expect(useDetailStore.getState().messages).toHaveLength(1);
    expect(useDetailStore.getState().isLoading).toBe(false);
  });

  it("when fetch fails immediately, also exposes state instead of silent blank", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    (invoke as unknown as { mockImplementation: (f: unknown) => void }).mockImplementation(
      (cmd: string) =>
        cmd === "get_messages_tail"
          ? Promise.reject(new Error("No agent source can handle path"))
          : Promise.resolve(undefined),
    );

    const { useDetailStore } = await import("./store");
    await useDetailStore.getState().open(session);

    expect(useDetailStore.getState().isLoading).toBe(false);
    // State exposure changed: failure goes to loadError (with reason), no longer pretends to be timeout stalled.
    expect(useDetailStore.getState().loadStalled).toBe(false);
    expect(useDetailStore.getState().loadError).toContain("No agent source");
  });
});

/**
 * The "system" theme setting must pass `null` to tauri, not the resolved dark/light.
 *
 * On macOS, tao's `set_theme` lands in `[NSApp setAppearance:]` (app-level, not per-window),
 * and WKWebView's `prefers-color-scheme` is resolved from that app appearance — exactly what
 * `getSystemTheme` reads back. So passing a concrete value locks the app into whatever we
 * just set: after the user picks dark once, switching back to "system" still reads our
 * soldered-in dark value, can't return to light even during daytime.
 */
describe("applyWindowTheme", () => {
  beforeEach(() => {
    vi.resetModules();
    winMock.setTheme.mockReset();
    winMock.setTheme.mockImplementation(async () => undefined);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** Stub the pair the latch runs through: the webview's media query answers
   *  from whatever appearance was last forced, and `null` releases it back to
   *  the OS — which here is light. Returns the data-theme values stamped. */
  function stubLatchedDarkWebview(): string[] {
    const stamped: string[] = [];
    let forced: "dark" | "light" | null = "dark";
    winMock.setTheme.mockImplementation(async (value: unknown) => {
      forced = value === null ? null : (value as "dark" | "light");
    });
    vi.stubGlobal("window", {
      matchMedia: (q: string) => ({
        // OS setting is light; only a forced dark appearance reads back dark.
        matches: q.includes("dark") && forced === "dark",
      }),
    });
    vi.stubGlobal("document", {
      documentElement: {
        setAttribute: (_k: string, v: string) => stamped.push(v),
      },
    });
    return stamped;
  }

  it("system setting passes null to release app lock and follow OS", async () => {
    const stamped = stubLatchedDarkWebview();

    const { applyWindowTheme } = await import("./store");
    await applyWindowTheme("system");

    expect(winMock.setTheme).toHaveBeenCalledWith(null);
    // Re-read after un-latching: the OS is light, so that's what gets stamped.
    expect(stamped.at(-1)).toBe("light");
  });

  it("explicit settings still solder their level to the window", async () => {
    stubLatchedDarkWebview();

    const { applyWindowTheme } = await import("./store");
    await applyWindowTheme("light");

    expect(winMock.setTheme).toHaveBeenCalledWith("light");
  });
});

/**
 * Deduplication contract for auto-popup daily reports.
 *
 * Two signals trigger maybePopupReport: a 1.5s make-up check after startup, and the
 * daily-report-ready event sent when the scheduler finishes writing the AI summary.
 * The startup check often arrives before the summary is written (scheduler's first cycle
 * waits 10s), so the "already popped" persistence flag must only be written after we
 * actually pop — write it early and the real event's outcome gets blocked, main path fails.
 */
describe("auto-popup daily report", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
  });

  async function setup(report: unknown) {
    const core = await import("@tauri-apps/api/core");
    (core.invoke as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      async () => report,
    );
    return await import("./store");
  }

  it("does not pop when summary not ready, does not write popped flag", async () => {
    const { useReportStore, REPORT_LAST_POPPED_KEY } = await setup({
      date: "2026-09-05",
      aiSummary: null,
    });
    const { getItem } = await import("./storage");

    await useReportStore.getState().maybePopupReport("2026-09-05");

    expect(useReportStore.getState().reportPopupDate).toBeNull();
    expect(getItem(REPORT_LAST_POPPED_KEY) ?? "").not.toBe("2026-09-05");
  });

  it("pops when summary ready, records the date", async () => {
    const { useReportStore, REPORT_LAST_POPPED_KEY } = await setup({
      date: "2026-09-05",
      aiSummary: "今天干了很多活",
    });
    const { getItem } = await import("./storage");

    await useReportStore.getState().maybePopupReport("2026-09-05");

    expect(useReportStore.getState().reportPopupDate).toBe("2026-09-05");
    expect(getItem(REPORT_LAST_POPPED_KEY)).toBe("2026-09-05");
  });

  it("does not pop twice on the same day", async () => {
    const { useReportStore } = await setup({
      date: "2026-09-05",
      aiSummary: "今天干了很多活",
    });

    await useReportStore.getState().maybePopupReport("2026-09-05");
    useReportStore.getState().closeReportPopup();
    await useReportStore.getState().maybePopupReport("2026-09-05");

    expect(useReportStore.getState().reportPopupDate).toBeNull();
  });

  it("does not pop when toggle is off", async () => {
    const { useReportStore, REPORT_AUTO_POPUP_KEY } = await setup({
      date: "2026-09-05",
      aiSummary: "今天干了很多活",
    });
    const { setFeatureState } = await import("./storage");
    setFeatureState(REPORT_AUTO_POPUP_KEY, "off");

    await useReportStore.getState().maybePopupReport("2026-09-05");

    expect(useReportStore.getState().reportPopupDate).toBeNull();
  });
});

/**
 * Auto-collapse: what the layout folds away on its own to make room for a wide
 * doc reader in a narrow pane, and puts back afterwards.
 *
 * The contract worth pinning down is the asymmetry — it restores exactly what
 * it took. A panel the user had already collapsed by hand must not spring open
 * when the reader closes, and nothing here may reach the settings store: an
 * auto-collapse is the layout coping, not a preference.
 */
describe("auto-collapse for a wide reader", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("collapses, then restores exactly what it collapsed", async () => {
    const { useUIStore } = await import("./store");
    const s = () => useUIStore.getState();

    s().autoCollapse("sidebar");
    s().autoCollapse("list");
    expect(s().sidebarCollapsed).toBe(true);
    expect(s().secondarySidebarCollapsed.list).toBe(true);

    s().autoRestore();
    expect(s().sidebarCollapsed).toBe(false);
    expect(s().secondarySidebarCollapsed.list).toBe(false);
  });

  it("leaves a panel the user collapsed by hand alone", async () => {
    const { useUIStore } = await import("./store");
    const s = () => useUIStore.getState();

    s().setSidebarCollapsed(true);
    s().autoCollapse("sidebar");
    s().autoRestore();

    expect(s().sidebarCollapsed).toBe(true);
  });

  it("stops owning a panel the user toggles mid-read", async () => {
    const { useUIStore } = await import("./store");
    const s = () => useUIStore.getState();

    s().autoCollapse("sidebar");
    s().setSidebarCollapsed(false);
    s().autoCollapse("list");
    s().setSecondarySidebar("list", false);
    s().autoRestore();

    expect(s().sidebarCollapsed).toBe(false);
    expect(s().secondarySidebarCollapsed.list).toBe(false);
    expect(s().autoCollapsed).toEqual({ sidebar: false, secondary: null });
  });

  it("never writes an auto-collapse to the settings store", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().autoCollapse("sidebar");
    expect(getItem("sidebar-collapsed")).not.toBe("true");
  });
});

describe("simplified mode", () => {
  beforeEach(() => vi.resetModules());

  it("persists the switch, enters Tasks, and restricts navigation until disabled", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");
    useUIStore.getState().setViewMode("wiki");
    useUIStore.getState().setSimplifiedMode(true);
    expect(getItem("simplified-mode")).toBe("true");
    expect(useUIStore.getState().viewMode).toBe("history");
    useUIStore.getState().setViewMode("artifacts");
    expect(useUIStore.getState().viewMode).toBe("artifacts");
    useUIStore.getState().setViewMode("report");
    expect(useUIStore.getState().viewMode).toBe("history");
    useUIStore.getState().setSimplifiedMode(false);
    useUIStore.getState().setViewMode("wiki");
    expect(useUIStore.getState().viewMode).toBe("wiki");
    expect(getItem("simplified-mode")).toBe("false");
  });

  /**
   * Host-provided default (backend's FLEET_SIMPLIFIED_MODE, sent via host_features).
   * It exists because browser builds store settings in localStorage each — a toggle
   * opened in one browser doesn't reach the next, only the host can speak for all clients.
   * Three invariants:
   *
   *   1. Host says on, client hasn't signaled ⇒ turn it on here, and land initial page
   *      on task page (mismatch leaves page stuck in a nav that doesn't have it);
   *   2. User explicitly turned off ⇒ host cannot override back to on;
   *   3. Cache host's answer for next sync read, update cache when host changes —
   *      otherwise it becomes an irreversible sticky toggle.
   */
  it("adopts the host default and caches it for the next load", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, simplifiedDefault: true });

    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");
    expect(useUIStore.getState().simplifiedMode).toBe(false);

    await useUIStore.getState().loadHostFeatures();
    expect(useUIStore.getState().simplifiedMode).toBe(true);
    expect(useUIStore.getState().viewMode).toBe("history");
    // Cached, not user choice: the latter must still be "hasn't signaled".
    expect(getItem("simplified-mode-host-default")).toBe("true");
    expect(getItem("simplified-mode")).toBe(null);
  });

  it("boots straight into simplified mode from cached host default", async () => {
    const { setItem } = await import("./storage");
    setItem("simplified-mode-host-default", "true");
    const { useUIStore } = await import("./store");
    expect(useUIStore.getState().simplifiedMode).toBe(true);
    expect(useUIStore.getState().viewMode).toBe("history");
  });

  it("lets explicit opt-out beat the host default", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, simplifiedDefault: true });

    const { setItem } = await import("./storage");
    setItem("simplified-mode", "false");
    setItem("viewMode", "wiki");
    const { useUIStore } = await import("./store");

    await useUIStore.getState().loadHostFeatures();
    expect(useUIStore.getState().simplifiedMode).toBe(false);
    expect(useUIStore.getState().viewMode).toBe("wiki");
  });

  it("drops the cache when the host stops asking for it", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false });

    const { setItem, getItem } = await import("./storage");
    setItem("simplified-mode-host-default", "true");
    const { useUIStore } = await import("./store");
    expect(useUIStore.getState().simplifiedMode).toBe(true);

    await useUIStore.getState().loadHostFeatures();
    expect(getItem("simplified-mode-host-default")).toBe(null);
    // Already on this time, don't change (user is looking at this screen); starting next load, won't default to on.
    expect(useUIStore.getState().simplifiedMode).toBe(true);
  });

  it("turns off simplified mode when host says off", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, simplifiedDefault: false });

    const { setItem, getItem } = await import("./storage");
    setItem("simplified-mode-host-default", "true");
    const { useUIStore } = await import("./store");
    expect(useUIStore.getState().simplifiedMode).toBe(true);

    await useUIStore.getState().loadHostFeatures();
    expect(useUIStore.getState().simplifiedMode).toBe(false);
    expect(getItem("simplified-mode-host-default")).toBe("false");
  });

  it.each(["wiki", "artifacts"])("restores simplified mode on boot from %s", async (view) => {
    const { setItem } = await import("./storage");
    setItem("simplified-mode", "true");
    setItem("viewMode", view);
    const { useUIStore } = await import("./store");
    expect(useUIStore.getState().simplifiedMode).toBe(true);
    expect(useUIStore.getState().viewMode).toBe(view === "artifacts" ? "artifacts" : "history");
  });
});

/**
 * The terminal page is controlled by backend's FLEET_TERMINAL on startup (core's feature_flags);
 * the frontend just decides whether this page exists based on the backend's answer. Three
 * invariants here: default must be off (and stays off if no answer), sessions already on terminal
 * page are sent back to home, and with it off any terminal-page request is a no-op — else the
 * user lands on a blank page with no nav items and no shell.
 */
describe("terminal feature toggle (host_features)", () => {
  beforeEach(() => vi.resetModules());

  it("defaults to off and stays off when the backend call fails", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockRejectedValueOnce(new Error("backend not ready"));

    const { useUIStore } = await import("./store");
    expect(useUIStore.getState().hostFeatures.terminal).toBe(false);

    await useUIStore.getState().loadHostFeatures();
    expect(useUIStore.getState().hostFeatures.terminal).toBe(false);
  });

  it("adopts the backend's answer when the flag is on", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: true });

    const { useUIStore } = await import("./store");
    await useUIStore.getState().loadHostFeatures();

    expect(useUIStore.getState().hostFeatures.terminal).toBe(true);
  });

  it("sends restored terminal page back to Work home when flag is off", async () => {
    const { setItem } = await import("./storage");
    setItem("viewMode", "terminal");
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false });

    const { useUIStore } = await import("./store");
    expect(useUIStore.getState().viewMode).toBe("terminal");

    await useUIStore.getState().loadHostFeatures();
    expect(useUIStore.getState().viewMode).toBe("history");
  });

  it("leaves a restored terminal page alone when the flag is on", async () => {
    const { setItem } = await import("./storage");
    setItem("viewMode", "terminal");
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: true });

    const { useUIStore } = await import("./store");
    await useUIStore.getState().loadHostFeatures();

    expect(useUIStore.getState().viewMode).toBe("terminal");
  });

  it("ignores open-in-terminal request while flag is off, honours it when on", async () => {
    const { useUIStore } = await import("./store");
    useUIStore.getState().setViewMode("files");

    useUIStore.getState().requestTerminalNav("/repo");
    expect(useUIStore.getState().viewMode).toBe("files");
    expect(useUIStore.getState().terminalNav).toBeNull();

    useUIStore.setState({ hostFeatures: { terminal: true } });
    useUIStore.getState().requestTerminalNav("/repo");
    expect(useUIStore.getState().viewMode).toBe("terminal");
    expect(useUIStore.getState().terminalNav?.workspacePath).toBe("/repo");
  });
});
