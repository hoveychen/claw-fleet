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
describe("nav sidebar yields to a wide reader", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("folds the widened sidebar in memory only and gives it back", async () => {
    const { setItem, getItem } = await import("./storage");
    setItem("sidebar-collapsed", "false");
    const { useUIStore } = await import("./store");

    useUIStore.getState().autoCollapse("sidebar");
    expect(useUIStore.getState().sidebarCollapsed).toBe(true);
    // The reader borrows the room; the saved preference stays "wide".
    expect(getItem("sidebar-collapsed")).toBe("false");

    useUIStore.getState().autoRestore();
    expect(useUIStore.getState().sidebarCollapsed).toBe(false);
  });

  it("does not reopen a sidebar the user folded by hand", async () => {
    const { setItem } = await import("./storage");
    setItem("sidebar-collapsed", "false");
    const { useUIStore } = await import("./store");

    useUIStore.getState().autoCollapse("sidebar");
    useUIStore.getState().setSidebarCollapsed(true);
    useUIStore.getState().autoRestore();
    expect(useUIStore.getState().sidebarCollapsed).toBe(true);
  });

  it("leaves an already-folded rail alone", async () => {
    const { setItem } = await import("./storage");
    setItem("sidebar-collapsed", "true");
    const { useUIStore } = await import("./store");

    useUIStore.getState().autoCollapse("sidebar");
    useUIStore.getState().autoRestore();
    expect(useUIStore.getState().sidebarCollapsed).toBe(true);
  });
});

describe("launchpad rail filters", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("round-trips the workspace filter", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().setHistoryWorkspaceFilter("/Users/x/workspace/maliang");

    expect(getItem("history-project-scope")).toBe("/Users/x/workspace/maliang");
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

  it("starts with the done section folded", async () => {
    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().homeCollapsedSections).toEqual(["done"]);
  });

  it("toggles a section fold and writes it through", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().toggleHomeSection("review");
    expect(useUIStore.getState().homeCollapsedSections).toEqual(["done", "review"]);
    expect(getItem("home-collapsed-sections")).toBe(JSON.stringify(["done", "review"]));

    // Toggling again unfolds it rather than piling up duplicates.
    useUIStore.getState().toggleHomeSection("review");
    expect(useUIStore.getState().homeCollapsedSections).toEqual(["done"]);
  });

  it("restores persisted folds and drops unknown states", async () => {
    const { setItem } = await import("./storage");
    setItem("home-collapsed-sections", JSON.stringify(["running", "bogus"]));

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().homeCollapsedSections).toEqual(["running"]);
  });

  it("groups by state until the reader picks project, and remembers it", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    expect(useUIStore.getState().homeGroupBy).toBe("state");
    useUIStore.getState().setHomeGroupBy("project");
    expect(useUIStore.getState().homeGroupBy).toBe("project");
    expect(getItem("home-group-by")).toBe("project");
  });

  it("restores the group-by choice and the folded projects", async () => {
    const { setItem } = await import("./storage");
    setItem("home-group-by", "project");
    setItem("home-collapsed-projects", JSON.stringify(["/work/a", 7]));

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().homeGroupBy).toBe("project");
    expect(useUIStore.getState().homeCollapsedProjects).toEqual(["/work/a"]);
    useUIStore.getState().toggleHomeProject("/work/a");
    expect(useUIStore.getState().homeCollapsedProjects).toEqual([]);
  });

  it("falls back to the default on a corrupt fold list", async () => {
    const { setItem } = await import("./storage");
    setItem("home-collapsed-sections", "{not json");

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().homeCollapsedSections).toEqual(["done"]);
  });
});

describe("primary nav page browsing context", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("keeps all page navigation state outside component lifetimes", async () => {
    const { useUIStore } = await import("./store");
    const { updateMainViewState } = useUIStore.getState();

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
      activeRootPath: "/repo/.worktrees/feature",
      showIgnored: true,
      activeFilePath: "src/main.rs",
    });
    updateMainViewState("mobile", { editingUrl: true, urlDraft: "wss://relay.example" });

    const state = useUIStore.getState().mainViewState;
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
      activeRootPath: "/repo/.worktrees/feature",
      showIgnored: true,
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
 * The Sessions page (list / gallery) is gone: every listed session is one Fleet
 * launched, so its detail lives on the Tasks page. A page value stored by an
 * older build must not strand the main area on a page nothing renders.
 */
describe("retired Sessions page", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("a stored list/gallery page boots onto the Tasks page", async () => {
    const { setItem } = await import("./storage");
    setItem("viewMode", "gallery");
    const { useUIStore } = await import("./store");
    expect(useUIStore.getState().viewMode).toBe("history");
  });

  it("a tray / notification click opens the session on the Tasks page", async () => {
    const { useUIStore, navigateToSessionDetail } = await import("./store");
    useUIStore.getState().setViewMode("audit");

    navigateToSessionDetail({ id: "sub-1", isSubagent: true } as SessionInfo);

    expect(useUIStore.getState().viewMode).toBe("history");
    expect(useUIStore.getState().openTaskNav?.sessionId).toBe("sub-1");
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
 * Deduplication contract for the auto-popup.
 *
 * Two signals trigger maybePopupReport: a 1.5s make-up check after startup, and the
 * daily-report-ready event the scheduler sends once a day has something that needs
 * the user's judgment. The startup check often arrives before the scheduler has
 * judged the day, so the "already popped" flag must only be written after we
 * actually pop — write it early and the real event gets blocked. A day with
 * nothing to judge never pops.
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

  it("does not pop when nothing needs judgment, does not write popped flag", async () => {
    const { useReportStore, REPORT_LAST_POPPED_KEY } = await setup({ date: "2026-09-05", drift: [], lessons: [], violations: [] });
    const { getItem } = await import("./storage");

    await useReportStore.getState().maybePopupReport("2026-09-05");

    expect(useReportStore.getState().reportPopupDate).toBeNull();
    expect(getItem(REPORT_LAST_POPPED_KEY) ?? "").not.toBe("2026-09-05");
  });

  it("pops when something needs judgment, records the date", async () => {
    const { useReportStore, REPORT_LAST_POPPED_KEY } = await setup({
      date: "2026-09-05",
      drift: [],
      lessons: [{ content: "c", reason: "r", workspaceName: "w", sessionId: "s1", evidenceSessionIds: ["s1", "s2"] }],
      violations: [],
    });
    const { getItem } = await import("./storage");

    await useReportStore.getState().maybePopupReport("2026-09-05");

    expect(useReportStore.getState().reportPopupDate).toBe("2026-09-05");
    expect(getItem(REPORT_LAST_POPPED_KEY)).toBe("2026-09-05");
  });

  it("does not pop twice on the same day", async () => {
    const { useReportStore } = await setup({
      date: "2026-09-05",
      drift: [],
      lessons: [{ content: "c", reason: "r", workspaceName: "w", sessionId: "s1", evidenceSessionIds: ["s1", "s2"] }],
      violations: [],
    });

    await useReportStore.getState().maybePopupReport("2026-09-05");
    useReportStore.getState().closeReportPopup();
    await useReportStore.getState().maybePopupReport("2026-09-05");

    expect(useReportStore.getState().reportPopupDate).toBeNull();
  });

  it("does not pop when toggle is off", async () => {
    const { useReportStore, REPORT_AUTO_POPUP_KEY } = await setup({
      date: "2026-09-05",
      drift: [],
      lessons: [{ content: "c", reason: "r", workspaceName: "w", sessionId: "s1", evidenceSessionIds: ["s1", "s2"] }],
      violations: [],
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

    s().autoCollapse("history");
    expect(s().secondarySidebarCollapsed.history).toBe(true);

    s().autoRestore();
    expect(s().secondarySidebarCollapsed.history).toBe(false);
  });

  it("leaves a panel the user collapsed by hand alone", async () => {
    const { useUIStore } = await import("./store");
    const s = () => useUIStore.getState();

    s().setSecondarySidebar("history", true);
    s().autoCollapse("history");
    s().autoRestore();

    expect(s().secondarySidebarCollapsed.history).toBe(true);
  });

  it("stops owning a panel the user toggles mid-read", async () => {
    const { useUIStore } = await import("./store");
    const s = () => useUIStore.getState();

    s().autoCollapse("history");
    s().setSecondarySidebar("history", false);
    s().autoRestore();

    expect(s().secondarySidebarCollapsed.history).toBe(false);
    expect(s().autoCollapsed).toEqual({ sidebar: false, secondary: null });
  });

  it("never writes an auto-collapse to the settings store", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().autoCollapse("history");
    expect(getItem("secondary-sidebar-collapsed") ?? "").not.toContain("history");
  });
});

/**
 * Each nav entry returns to the page of its section that was open last, so
 * flipping Library -> Tasks -> Library lands back on Deliverables rather than
 * on Docs every time.
 */
describe("nav section memory", () => {
  beforeEach(() => vi.resetModules());

  it("records the last page of each section", async () => {
    const { useUIStore } = await import("./store");
    const s = () => useUIStore.getState();
    s().setViewMode("artifacts");
    s().setViewMode("memory");
    s().setViewMode("history");
    expect(s().navLastBySection).toMatchObject({
      library: "artifacts",
      projects: "memory",
      tasks: "history",
    });
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

  it("opens a clicked path on its project's code tab, worktrees folded onto the repo", async () => {
    const { useUIStore } = await import("./store");
    useUIStore.getState().requestFileNav({
      workspacePath: "/repo/.worktrees/feat",
      absPath: "/repo/.worktrees/feat/src/main.rs",
      line: null,
    });
    const s = useUIStore.getState();
    expect(s.viewMode).toBe("projects");
    expect(s.mainViewState.projects).toMatchObject({ selectedPath: "/repo", tab: "code" });
    expect(s.fileNav?.absPath).toBe("/repo/.worktrees/feat/src/main.rs");
  });

  it("ignores open-in-terminal request while flag is off, honours it when on", async () => {
    const { useUIStore } = await import("./store");
    useUIStore.getState().setViewMode("projects");

    useUIStore.getState().requestTerminalNav("/repo");
    expect(useUIStore.getState().viewMode).toBe("projects");
    expect(useUIStore.getState().terminalNav).toBeNull();

    useUIStore.setState({ hostFeatures: { terminal: true } });
    useUIStore.getState().requestTerminalNav("/repo");
    expect(useUIStore.getState().viewMode).toBe("terminal");
    expect(useUIStore.getState().terminalNav?.workspacePath).toBe("/repo");
  });
});

describe("lobster fleet setting", () => {
  beforeEach(() => vi.resetModules());

  it("is off by default and keeps the page out of reach until switched on", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");
    expect(useUIStore.getState().lobsterFleetEnabled).toBe(false);
    useUIStore.getState().setViewMode("lobsters");
    expect(useUIStore.getState().viewMode).toBe("history");
    useUIStore.getState().setLobsterFleetEnabled(true);
    expect(getItem("lobster-fleet-enabled")).toBe("true");
    useUIStore.getState().setViewMode("lobsters");
    expect(useUIStore.getState().viewMode).toBe("lobsters");
    // Switching it off while on the page drops back to Tasks.
    useUIStore.getState().setLobsterFleetEnabled(false);
    expect(useUIStore.getState().viewMode).toBe("history");
  });

  it("does not boot into the page when the setting is off", async () => {
    const { setItem } = await import("./storage");
    setItem("viewMode", "lobsters");
    setItem("lobster-fleet-enabled", "false");
    const { useUIStore } = await import("./store");
    expect(useUIStore.getState().viewMode).toBe("history");
  });
});
