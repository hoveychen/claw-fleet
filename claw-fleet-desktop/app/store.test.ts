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
 * 侧栏顶部的 舰队 / 工作 tab。The active tab is *derived* from `viewMode`
 * (navGroupOf) rather than stored beside it, so the invariant worth testing is
 * the other half: each tab remembers the page you left it on, and every path
 * that moves the main area — the nav itself and the three cross-page requests
 * (file link, tray click, schedule → new session) — feeds that memory. A path
 * that wrote `viewMode` directly would leave its tab restoring a stale page.
 */
describe("侧栏模式 tab（舰队 / 工作）", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("切 tab 回到该 tab 上次停留的页面", async () => {
    const { useUIStore } = await import("./store");
    const ui = () => useUIStore.getState();

    ui().setViewMode("audit"); // 舰队
    ui().setViewMode("wiki"); // 工作

    ui().setNavGroup("fleet");
    expect(ui().viewMode).toBe("audit");

    ui().setNavGroup("work");
    expect(ui().viewMode).toBe("wiki");
  });

  it("首次进入某个 tab 落在它的主页", async () => {
    const { useUIStore } = await import("./store");

    useUIStore.getState().setNavGroup("work");
    expect(useUIStore.getState().viewMode).toBe("history");
  });

  it("点当前所在 tab 不换页面", async () => {
    const { useUIStore } = await import("./store");

    useUIStore.getState().setViewMode("plans");
    useUIStore.getState().setNavGroup("work");

    expect(useUIStore.getState().viewMode).toBe("plans");
  });

  it("绕过 nav 的跨页跳转同样记进 tab 记忆", async () => {
    const { useUIStore } = await import("./store");
    const ui = () => useUIStore.getState();

    // 先让 工作 tab 的记忆停在 计划树，这样「主页兜底」和「真的记账了」两种
    // 结果不再撞成同一个值 —— 否则跳到 history 的断言不记账也能通过。
    ui().setViewMode("plans");

    // 托盘/通知点击 → 任务页（工作 tab），走的是 requestOpenTask 而非 setViewMode。
    ui().requestOpenTask("sess-1");
    expect(ui().viewMode).toBe("history");

    ui().setNavGroup("fleet");
    expect(ui().viewMode).toBe("gallery");

    // 若 requestOpenTask 没记账，这里会退回上一页 plans。
    ui().setNavGroup("work");
    expect(ui().viewMode).toBe("history");

    // 文件链接点击（requestFileNav）是同一类绕过 nav 的路径。
    ui().requestFileNav({ workspacePath: "/w", absPath: "/w/a.ts", line: 3 });
    ui().setNavGroup("fleet");
    ui().setNavGroup("work");
    expect(ui().viewMode).toBe("files");
  });

  it("把每个 tab 的最后一页写进设置存储", async () => {
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");

    useUIStore.getState().setViewMode("memory");
    useUIStore.getState().setViewMode("files");

    expect(JSON.parse(getItem("nav-group-last-view")!)).toMatchObject({
      fleet: "memory",
      work: "files",
    });
  });

  it("丢弃已不属于该 tab 的存量页面，回落到主页", async () => {
    const { setItem } = await import("./storage");
    // wiki 现在归 工作 tab；一份把它记在 舰队 名下的旧数据不该让 舰队 打开它。
    setItem("nav-group-last-view", JSON.stringify({ fleet: "wiki", work: "nonsense" }));

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().lastViewByNavGroup).toEqual({
      fleet: "gallery",
      work: "history",
    });
  });

  it("认得改名前写下的 steward 记忆", async () => {
    const { setItem } = await import("./storage");
    // 这个 tab 的代号在 2026-08 从 steward 改成 fleet。已经在用的机器上，磁盘里
    // 存的还是旧键；读不认它的话，老板的「舰队 tab 上次停在记忆页」会静默丢掉。
    setItem("nav-group-last-view", JSON.stringify({ steward: "memory", work: "files" }));

    const { useUIStore } = await import("./store");

    expect(useUIStore.getState().lastViewByNavGroup).toEqual({
      fleet: "memory",
      work: "files",
    });
  });

  it("新键优先于同时存在的旧键", async () => {
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

/**
 * 「对话」Tab 永远卡在「加载中…」。
 *
 * 受控复现(2026-08-17,P1 harness + `kill -STOP` 掉 dsh web):后端一旦不
 * 响应,`get_messages_tail` 就永远不 settle,而详情页对这次取数**没有任何
 * 期限** —— 标题 / tok / 四个 Tab 照常渲染,只有对话区一直是「加载中…」,
 * 80s 不动,单条 /messages 120s 未返回。老板看到的正是这一幕。
 *
 * 契约:超过期限还没拿到 transcript 就必须停下转圈、把状态摊开(stalled),
 * 让 UI 能给出解释和重试;迟到的结果照常渲染,不能因为标了 stalled 就丢掉。
 */
describe("详情取数期限", () => {
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

  it("后端永不返回时,到期停止转圈并标记 stalled", async () => {
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

  it("迟到的 transcript 仍会渲染并清掉 stalled", async () => {
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
   * 同一幕的第二个入口,期限机制盖不到的那个。
   *
   * `open()` 里 `isLoading` 要等两个 await 都回来:`get_messages_tail`(有
   * 20s 期限保护)之后还有一个 `start_watching_session`。后者取的是 backend
   * 的**写锁**,而 `get_messages_tail` / `read_live_thinking` 取读锁 —— 活跃
   * 会话上这两个轮询分别是 1.5s / 700ms 一发,所以写锁这一步的等待时间由别人
   * 的读锁决定,不由 transcript 取数决定。
   *
   * 而 `withStallWatch` 只包了第一个 await:transcript 一旦取回来,期限就被
   * disarm 了。于是卡在第二个 await 上的症状是**永久「加载中…」且不出重试
   * 按钮** —— 消息其实已经在手里,只是没被 set 进 store。
   *
   * 契约:watcher 注册不在渲染关键路径上。transcript 到手就渲染,注册失败或
   * 迟到都不许扣着已经取到的消息。
   */
  it("watcher 注册永不返回时,已取到的 transcript 仍须渲染", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    (invoke as unknown as { mockImplementation: (f: unknown) => void }).mockImplementation(
      (cmd: string) => {
        if (cmd === "get_messages_tail") return Promise.resolve([{ type: "user" }]);
        // 写锁一直拿不到 —— 命令发出去了,永远不 settle。
        if (cmd === "start_watching_session") return new Promise(() => {});
        return Promise.resolve(undefined);
      },
    );

    const { useDetailStore } = await import("./store");
    // 不 await open():它本身就挂在那个永不 settle 的注册上,await 会把这个
    // 测试变成超时而不是一次干净的断言失败。
    void useDetailStore.getState().open(session);
    for (let i = 0; i < 20; i++) await Promise.resolve();

    expect(useDetailStore.getState().messages).toHaveLength(1);
    expect(useDetailStore.getState().isLoading).toBe(false);
  });

  it("取数直接失败时也摊开状态,而不是留一片静默的空白", async () => {
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
    expect(useDetailStore.getState().loadStalled).toBe(true);
  });
});

/**
 * 主题「系统」档必须把 `null` 交给 tauri，而不是解析后的 dark/light。
 *
 * macOS 上 tao 的 `set_theme` 落到 `[NSApp setAppearance:]`（application 级，
 * 不是单窗口），而 WKWebView 的 `prefers-color-scheme` 正是从这个 app
 * appearance 解析出来的 —— 也就是 `getSystemTheme` 读回来的那个值。所以传
 * 具体值会把 app 焊死在自己刚设的那一档：老板选过一次暗色之后，再切回
 * 「系统」读到的仍是我们自己焊上去的 dark，白天也回不到亮色。
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

  it("「系统」档传 null，让 app 松开回到跟随系统", async () => {
    const stamped = stubLatchedDarkWebview();

    const { applyWindowTheme } = await import("./store");
    await applyWindowTheme("system");

    expect(winMock.setTheme).toHaveBeenCalledWith(null);
    // Re-read after un-latching: the OS is light, so that's what gets stamped.
    expect(stamped.at(-1)).toBe("light");
  });

  it("显式档位仍然把自己那一档焊给窗口", async () => {
    stubLatchedDarkWebview();

    const { applyWindowTheme } = await import("./store");
    await applyWindowTheme("light");

    expect(winMock.setTheme).toHaveBeenCalledWith("light");
  });
});
