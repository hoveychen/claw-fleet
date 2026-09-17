import { describe, expect, it } from "vitest";
import {
  applyFrozenOrder,
  buildRenderItems,
  workspaceFilterValue,
  groupTaskSections,
  statusTone,
} from "./TasksView";
import type { SessionInfo } from "../types";
import type { WithDevice } from "../deviceRuntime";

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
      { chatPathOf: () => CHAT, multiDevice: false },
    );
    expect(secs.map((s) => s.path)).toEqual([CHAT, "/work/repo"]);
  });

  it("keeps the incoming row order otherwise — the freeze must survive grouping", () => {
    const secs = groupTaskSections(
      [row("a", "/work/a", "a"), row("b", "/work/b", "b"), row("c", "/work/a", "a")],
      { chatPathOf: () => null, multiDevice: false },
    );
    expect(secs.map((s) => s.path)).toEqual(["/work/a", "/work/b"]);
    expect(secs[0].sessions.map((s) => s.id)).toEqual(["a", "c"]);
  });

  // Folders are stable directory list: zebra's newest task (first in input list) can't push it to the front.
  it("orders folders alphabetically, not by their first member's position", () => {
    const secs = groupTaskSections(
      [row("z", "/work/zebra", "zebra"), row("a", "/work/apple", "apple")],
      { chatPathOf: () => null, multiDevice: false },
    );
    expect(secs.map((s) => s.path)).toEqual(["/work/apple", "/work/zebra"]);
  });

  it("folds a worktree checkout into its repository section", () => {
    const secs = groupTaskSections(
      [row("a", "/work/repo", "repo"), row("b", "/work/repo/.worktrees/fix", "repo")],
      { chatPathOf: () => null, multiDevice: false },
    );
    expect(secs).toHaveLength(1);
    expect(secs[0].path).toBe("/work/repo");
  });

  // Same path /repos/foo on two devices are two different repositories; merging into one section is confusing.
  it("splits the same path on two devices, labelling each", () => {
    const rows = [
      { ...row("a", "/repos/foo", "foo"), deviceId: "dev-a" },
      { ...row("b", "/repos/foo", "foo"), deviceId: "dev-b" },
    ] as Array<WithDevice<SessionInfo>>;
    const secs = groupTaskSections(rows, {
      chatPathOf: () => null,
      multiDevice: true,
      deviceLabelOf: (id) => (id === "dev-a" ? "MBP" : "Studio"),
    });
    expect(secs.map((s) => s.key)).toEqual(["dev-a::/repos/foo", "dev-b::/repos/foo"]);
    expect(secs.map((s) => s.name)).toEqual(["MBP · foo", "Studio · foo"]);
  });

  // Remote host's chat folder is its own home path (`/root/.fleet/chat`): comparing with local path never matches,
  // so that device's Chat section sinks in the middle of projects (actual symptom).
  it("pins each device's own chat folder, not just the active device's", () => {
    const REMOTE_CHAT = "/root/.fleet/chat";
    const rows = [
      { ...row("a", "/work/repo", "repo"), deviceId: "dev-a" },
      { ...row("b", REMOTE_CHAT, "Chat"), deviceId: "dev-b" },
      { ...row("c", CHAT, "Chat"), deviceId: "dev-a" },
    ] as Array<WithDevice<SessionInfo>>;
    const secs = groupTaskSections(rows, {
      chatPathOf: (id) => (id === "dev-a" ? CHAT : REMOTE_CHAT),
      multiDevice: true,
    });
    expect(secs.slice(0, 2).map((s) => s.path).sort()).toEqual([REMOTE_CHAT, CHAT].sort());
    expect(secs[2].path).toBe("/work/repo");
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
    // Distinct ids: the tone mapping is what's under test, and the anti-flicker
    // latch is per session — one id would carry the quiet state across.
    expect(statusTone(row({ id: "w", status: "executing", procAlive: true }))).toBe("working");
    expect(statusTone(row({ id: "i", status: "waitingInput", procAlive: true }))).toBe("waiting");
  });

  it("gives a watch-parked row its own tone instead of no dot", () => {
    // Its process is gone, so the old logic fell all the way through to
    // `return null` — no dot, indistinguishable from an ended row, while a Fleet
    // timer was still going to resume it once the condition fires.
    expect(statusTone(row({ id: "wt", status: "watching", procAlive: false }))).toBe("watching");
  });

  it("does not flick back to working on a single sparse write", () => {
    // Same flicker the desktop row had: a session parked on one long tool call
    // writes a line every few minutes, each write pushes the status back to a
    // live one for its hard window, and the dot alternated quiet → working →
    // quiet. One lone write must not win the bright tone back.
    const now = Date.now();
    const id = "flicker-1";
    expect(
      statusTone(row({ id, status: "idle", procAlive: true, lastActivityMs: now - 200_000 })),
    ).toBe("quiet");
    expect(
      statusTone(row({ id, status: "executing", procAlive: true, lastActivityMs: now - 1_000 })),
    ).toBe("quiet");
  });

  it("goes back to working once two writes land close together", () => {
    const now = Date.now();
    const id = "recover-1";
    statusTone(row({ id, status: "idle", procAlive: true, lastActivityMs: now - 200_000 }));
    statusTone(row({ id, status: "executing", procAlive: true, lastActivityMs: now - 20_000 }));
    expect(
      statusTone(row({ id, status: "executing", procAlive: true, lastActivityMs: now - 1_000 })),
    ).toBe("working");
  });
});

// Each row in the merged list must carry which device it belongs to all the way through: after collapsing
// into relay groups and extracting members, deviceId must not drop — losing it means guessing, and guessing
// wrong means using another device's transport to fetch a session it doesn't even know about.
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
    // React key must also split; otherwise both groups share the same key.
    expect(items[0].key).not.toBe(items[1].key);
  });
});

// Same path /repos/foo on two devices are two different repositories — if section key is path-only, both
// devices' sessions merge into one folder section and clicking in is confusing (section splitting itself in groupTaskSections).
describe("workspaceFilterValue", () => {
  it("scopes the section key by device when several are paired", () => {
    expect(workspaceFilterValue("dev-a", "/repos/foo", true)).toBe("dev-a::/repos/foo");
    // Single device stays as-is: old drafts stored bare path, value change silently breaks filter.
    expect(workspaceFilterValue("dev-a", "/repos/foo", false)).toBe("/repos/foo");
  });
});

// Task bar defaults to lastActivityMs descending; desktop pushes full snapshots every few seconds. If list
// reorders while scrolling, it swaps the card under the finger for a different one — finger lifts and
// taps the wrong session. Must freeze order during scroll (and for 5 seconds after stopping).
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
    // Freeze captured a, b, c on screen; new snapshot pushed c to the front.
    const frozen = ["d::a", "d::b", "d::c"];
    const resorted = [row("d", "c"), row("d", "a"), row("d", "b")];
    expect(keys(applyFrozenOrder(resorted, frozen))).toEqual(["d::a", "d::b", "d::c"]);
  });

  it("appends sessions born after the freeze at the bottom, never in the middle", () => {
    // New session should naturally sort first; inserting it would push every card down one slot.
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
