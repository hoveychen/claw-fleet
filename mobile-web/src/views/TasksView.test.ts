import { describe, expect, it } from "vitest";
import {
  applyFrozenOrder,
  buildRenderItems,
  workspaceFilterValue,
  groupOpenReadTargets,
  matchesWorkspaceFilter,
  groupTaskSections,
  statusTone,
} from "./TasksView";
import type { SessionInfo } from "../types";
import type { WithDevice } from "../deviceRuntime";

/**
 * The tasks list mixes chat sessions with project ones. The chat workspace path
 * comes from the desktop over the relay (`chat_workspace`), so the phone can be
 * asked to filter by it before — or without ever — learning it.
 */
function session(workspacePath: string): SessionInfo {
  return { id: "s", workspacePath, workspaceName: "n" } as unknown as SessionInfo;
}

describe("matchesWorkspaceFilter", () => {
  const CHAT = "/Users/foo/.fleet/chat";
  const chat = session(CHAT);
  const repo = session("/Users/foo/repo");

  it("passes everything under 全部目录", () => {
    expect(matchesWorkspaceFilter(chat, "")).toBe(true);
    expect(matchesWorkspaceFilter(repo, "")).toBe(true);
  });

  it("narrows to one folder", () => {
    expect(matchesWorkspaceFilter(repo, "/Users/foo/repo")).toBe(true);
    expect(matchesWorkspaceFilter(chat, "/Users/foo/repo")).toBe(false);
  });

  // 选项值是仓库根,而会话可能跑在 `<repo>/.worktrees/<task>` 里 —— 精确比路径
  // 会把它从这个目录的视图里静默漏掉。
  it("matches a worktree checkout against its repo root", () => {
    const wt = session("/Users/foo/repo/.worktrees/fix-bug");
    expect(matchesWorkspaceFilter(wt, "/Users/foo/repo")).toBe(true);
  });
});

describe("groupTaskSections", () => {
  const CHAT = "/Users/foo/.fleet/chat";
  function row(id: string, workspacePath: string, workspaceName: string) {
    return {
      id,
      workspacePath,
      workspaceName,
      deviceId: "d1",
    } as unknown as WithDevice<SessionInfo>;
  }

  it("pins the chat folder to the top however stale it is", () => {
    const secs = groupTaskSections(
      [row("a", "/work/repo", "repo"), row("b", CHAT, "Chat")],
      { chatPath: CHAT, multiDevice: false },
    );
    expect(secs.map((s) => s.path)).toEqual([CHAT, "/work/repo"]);
  });

  it("keeps the incoming order otherwise — the freeze must survive grouping", () => {
    const secs = groupTaskSections(
      [row("a", "/work/a", "a"), row("b", "/work/b", "b"), row("c", "/work/a", "a")],
      { chatPath: null, multiDevice: false },
    );
    expect(secs.map((s) => s.path)).toEqual(["/work/a", "/work/b"]);
    expect(secs[0].sessions.map((s) => s.id)).toEqual(["a", "c"]);
  });

  it("folds a worktree checkout into its repository section", () => {
    const secs = groupTaskSections(
      [row("a", "/work/repo", "repo"), row("b", "/work/repo/.worktrees/fix", "repo")],
      { chatPath: null, multiDevice: false },
    );
    expect(secs).toHaveLength(1);
    expect(secs[0].path).toBe("/work/repo");
  });

  // 两台机器上同路径的 /repos/foo 是两个不同的仓库,合成一个分区点进去是混的。
  it("splits the same path on two devices, labelling each", () => {
    const rows = [
      { ...row("a", "/repos/foo", "foo"), deviceId: "dev-a" },
      { ...row("b", "/repos/foo", "foo"), deviceId: "dev-b" },
    ] as Array<WithDevice<SessionInfo>>;
    const secs = groupTaskSections(rows, {
      chatPath: null,
      multiDevice: true,
      deviceLabelOf: (id) => (id === "dev-a" ? "MBP" : "Studio"),
    });
    expect(secs.map((s) => s.key)).toEqual(["dev-a::/repos/foo", "dev-b::/repos/foo"]);
    expect(secs.map((s) => s.name)).toEqual(["MBP · foo", "Studio · foo"]);
  });
});

/** A session with an optional handoff-chain stamp. */
function chainSession(
  id: string,
  chain?: { chainId: string; hop: number; chainLen: number },
): SessionInfo {
  return {
    id,
    workspacePath: "/w",
    workspaceName: "w",
    handoff: chain ?? null,
  } as unknown as SessionInfo;
}

describe("buildRenderItems", () => {
  it("is a pure single-list passthrough when grouping is off", () => {
    const rows = [chainSession("a", { chainId: "c", hop: 2, chainLen: 2 }), chainSession("b")];
    const items = buildRenderItems(rows, false);
    expect(items).toHaveLength(2);
    expect(items.every((i) => i.kind === "single")).toBe(true);
  });

  it("collapses ≥2 members sharing a chainId into one group, tip = highest hop", () => {
    const rows = [
      chainSession("tip", { chainId: "c", hop: 3, chainLen: 3 }),
      chainSession("mid", { chainId: "c", hop: 2, chainLen: 3 }),
      chainSession("solo"),
    ];
    const items = buildRenderItems(rows, true);
    expect(items).toHaveLength(2);
    const group = items[0];
    expect(group.kind).toBe("group");
    if (group.kind === "group") {
      expect(group.chainId).toBe("c");
      expect(group.chainLen).toBe(3);
      expect(group.tip.id).toBe("tip");
      // Newest hop first.
      expect(group.members.map((m) => m.id)).toEqual(["tip", "mid"]);
    }
    expect(items[1].kind).toBe("single");
  });

  it("does NOT group a chain with only one member present (renders as a plain row)", () => {
    const rows = [chainSession("lonely", { chainId: "c", hop: 5, chainLen: 5 })];
    const items = buildRenderItems(rows, true);
    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe("single");
  });

  it("treats a chainLen<=1 stamp as a standalone session", () => {
    const rows = [chainSession("x", { chainId: "c", hop: 1, chainLen: 1 })];
    const items = buildRenderItems(rows, true);
    expect(items[0].kind).toBe("single");
  });
});

/**
 * A collapsed relay group's header aggregates its unread dot over the whole
 * chain, but opening the header only navigates to (and dwells) the tip. The
 * other hops never get a detail dwell of their own, so `groupOpenReadTargets`
 * clears them on open — otherwise a non-tip unread hop keeps the group's dot
 * lit after the user plainly opened it (the bug this covers). The tip itself is
 * excluded so its own detail dwell still governs it.
 */
function readState(id: string, unread: boolean): SessionInfo {
  // isSessionUnread(s) = s.lastActivityMs > (s.lastReadMs ?? 0)
  return {
    id,
    workspacePath: "/w",
    workspaceName: "w",
    lastActivityMs: 100,
    lastReadMs: unread ? 0 : 100,
  } as unknown as SessionInfo;
}

describe("groupOpenReadTargets", () => {
  it("returns the unread non-tip hops, excluding the tip and already-read hops", () => {
    const tip = readState("tip", true);
    const chain = [tip, readState("hop2", true), readState("hop1", false)];
    expect(groupOpenReadTargets(tip, chain).map((m) => m.id)).toEqual(["hop2"]);
  });

  it("returns nothing when only the tip is unread", () => {
    const tip = readState("tip", true);
    const chain = [tip, readState("hop2", false), readState("hop1", false)];
    expect(groupOpenReadTargets(tip, chain)).toEqual([]);
  });

  it("returns nothing when the whole chain is already read", () => {
    const tip = readState("tip", false);
    expect(groupOpenReadTargets(tip, [tip, readState("hop2", false)])).toEqual([]);
  });
});

describe("statusTone quiet-alive", () => {
  const row = (over: Partial<SessionInfo>) =>
    ({ id: "s", workspacePath: "/w", workspaceName: "n", status: "idle", ...over }) as SessionInfo;

  it("keeps a dot on a live process whose transcript aged out to idle", () => {
    // The desktop's `isQuietAlive`, mirrored: status is derived from transcript
    // age alone, so a session sitting on one long tool call reads `idle` while
    // its process runs on. Dropping the dot would contradict the composer,
    // which offers to queue a follow-up for that very session.
    expect(statusTone(row({ status: "idle", procAlive: true }))).toBe("quiet");
  });

  it("still shows nothing once the process is gone", () => {
    expect(statusTone(row({ status: "idle", procAlive: false }))).toBe(null);
    expect(statusTone(row({ status: "idle" }))).toBe(null);
  });

  it("does not dim a genuinely working row down to quiet", () => {
    expect(statusTone(row({ status: "executing", procAlive: true }))).toBe("working");
    expect(statusTone(row({ status: "waitingInput", procAlive: true }))).toBe("waiting");
  });
});

// 合并列表里的每一条都必须一路带着它属于哪一台设备：折叠成接力组、再从组里
// 取出成员之后，deviceId 不能在中途掉队 —— 掉了就只能猜，而猜错就是拿另一台
// 的 transport 去拉一条它根本不认识的会话。
describe("device tag survives grouping", () => {
  const hop = (id: string, deviceId: string, chainId: string, n: number) =>
    ({
      id,
      deviceId,
      workspacePath: "/w",
      workspaceName: "n",
      status: "idle",
      handoff: { chainId, chainLen: 2, hop: n },
    }) as unknown as SessionInfo & { deviceId: string };

  it("carries deviceId through a collapsed relay group", () => {
    const items = buildRenderItems(
      [hop("s2", "dev-a", "c1", 2), hop("s1", "dev-a", "c1", 1)],
      true,
    );
    expect(items).toHaveLength(1);
    const group = items[0];
    if (group.kind !== "group") throw new Error("expected a group");
    expect(group.tip.deviceId).toBe("dev-a");
    expect(group.members.map((m) => m.deviceId)).toEqual(["dev-a", "dev-a"]);
  });

  it("never folds two devices' same-id chains into one group", () => {
    const items = buildRenderItems(
      [
        hop("s2", "dev-a", "c1", 2),
        hop("s1", "dev-a", "c1", 1),
        hop("s2", "dev-b", "c1", 2),
        hop("s1", "dev-b", "c1", 1),
      ],
      true,
    );
    expect(items).toHaveLength(2);
    for (const item of items) {
      if (item.kind !== "group") throw new Error("expected two groups");
      const devices = new Set(item.members.map((m) => m.deviceId));
      expect(devices.size).toBe(1);
    }
    // React key 也必须分家,否则两组共用一个 key。
    expect(items[0].key).not.toBe(items[1].key);
  });
});

// 两台机器上同路径的 /repos/foo 是两个不同的仓库。目录筛选若只按路径匹配，
// 选中一个就会把另一台同名目录的会话也筛进来 —— 列表看着对，点进去却是另一台
// 的会话。
describe("workspace filter across devices", () => {
  const row = (deviceId: string, workspacePath: string) =>
    ({ id: "s", deviceId, workspacePath, workspaceName: "foo", status: "idle" }) as unknown as
      SessionInfo & { deviceId: string };

  it("scopes the filter value by device when several are paired", () => {
    expect(workspaceFilterValue("dev-a", "/repos/foo", true)).toBe("dev-a::/repos/foo");
    // 单设备保持原样：老草稿里存的是裸路径，值一变筛选就会静默失效。
    expect(workspaceFilterValue("dev-a", "/repos/foo", false)).toBe("/repos/foo");
  });

  it("does not let one device's folder pick in the other device's sessions", () => {
    const filter = workspaceFilterValue("dev-a", "/repos/foo", true);
    expect(matchesWorkspaceFilter(row("dev-a", "/repos/foo"), filter)).toBe(true);
    expect(matchesWorkspaceFilter(row("dev-b", "/repos/foo"), filter)).toBe(false);
  });

  it("still matches by bare path for a single-device filter value", () => {
    expect(matchesWorkspaceFilter(row("dev-a", "/repos/foo"), "/repos/foo")).toBe(true);
  });
});

// 任务栏默认按 lastActivityMs 降序,而桌面端每隔几秒就推一次全量快照。手指还
// 在列表上滑的时候一次重排,会把手指底下那张卡换成另一张 —— 抬手点下去开的是
// 别的会话。滚动期间(以及停下后的 5 秒内)必须冻住顺序。
describe("applyFrozenOrder", () => {
  const row = (deviceId: string, id: string) =>
    ({ id, deviceId, workspacePath: "/w", workspaceName: "n", status: "idle" }) as unknown as
      SessionInfo & { deviceId: string };
  const keys = (rows: Array<{ deviceId?: string; id: string }>) =>
    rows.map((r) => `${r.deviceId ?? ""}::${r.id}`);

  it("passes rows through untouched when nothing is frozen", () => {
    const rows = [row("d", "a"), row("d", "b")];
    expect(keys(applyFrozenOrder(rows, null))).toEqual(["d::a", "d::b"]);
  });

  it("holds the frozen order even after the fresh sort flipped the rows", () => {
    // 冻结时屏幕上是 a、b、c;新快照把 c 顶到了最前。
    const frozen = ["d::a", "d::b", "d::c"];
    const resorted = [row("d", "c"), row("d", "a"), row("d", "b")];
    expect(keys(applyFrozenOrder(resorted, frozen))).toEqual(["d::a", "d::b", "d::c"]);
  });

  it("appends sessions born after the freeze at the bottom, never in the middle", () => {
    // 新会话按自然顺序本该排第一,插进去会把每张卡都顶下一格。
    const frozen = ["d::a", "d::b"];
    const rows = [row("d", "new"), row("d", "a"), row("d", "b")];
    expect(keys(applyFrozenOrder(rows, frozen))).toEqual(["d::a", "d::b", "d::new"]);
  });

  it("keeps several post-freeze arrivals in their own natural order", () => {
    const frozen = ["d::a"];
    const rows = [row("d", "n2"), row("d", "n1"), row("d", "a")];
    expect(keys(applyFrozenOrder(rows, frozen))).toEqual(["d::a", "d::n2", "d::n1"]);
  });

  it("silently drops frozen keys whose session is gone (filtered out or ended)", () => {
    const frozen = ["d::a", "d::gone", "d::b"];
    expect(keys(applyFrozenOrder([row("d", "b"), row("d", "a")], frozen))).toEqual([
      "d::a",
      "d::b",
    ]);
  });

  it("scopes the frozen key by device so two machines' same id never swap places", () => {
    const frozen = ["dev-b::s", "dev-a::s"];
    const rows = [row("dev-a", "s"), row("dev-b", "s")];
    const out = applyFrozenOrder(rows, frozen);
    expect(keys(out)).toEqual(["dev-b::s", "dev-a::s"]);
  });
});
