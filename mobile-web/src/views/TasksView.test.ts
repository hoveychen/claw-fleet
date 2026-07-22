import { describe, expect, it } from "vitest";
import {
  buildRenderItems,
  CHAT_HIDDEN,
  CHAT_ONLY,
  groupOpenReadTargets,
  matchesWorkspaceFilter,
} from "./TasksView";
import type { SessionInfo } from "../types";

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

  it("passes everything when no workspace is picked, chat included", () => {
    expect(matchesWorkspaceFilter(chat, "", CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(repo, "", CHAT)).toBe(true);
  });

  it("keeps only chat sessions under CHAT_ONLY", () => {
    expect(matchesWorkspaceFilter(chat, CHAT_ONLY, CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(repo, CHAT_ONLY, CHAT)).toBe(false);
  });

  it("drops chat sessions under CHAT_HIDDEN", () => {
    expect(matchesWorkspaceFilter(chat, CHAT_HIDDEN, CHAT)).toBe(false);
    expect(matchesWorkspaceFilter(repo, CHAT_HIDDEN, CHAT)).toBe(true);
  });

  it("still matches a plain workspace path", () => {
    expect(matchesWorkspaceFilter(repo, "/Users/foo/repo", CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(chat, "/Users/foo/repo", CHAT)).toBe(false);
  });

  it("degrades to 'all' when the desktop never sent a chat path", () => {
    // An older desktop doesn't know the `chat_workspace` method. Showing every
    // session beats blanking the list.
    for (const s of [chat, repo]) {
      expect(matchesWorkspaceFilter(s, CHAT_ONLY, null)).toBe(true);
      expect(matchesWorkspaceFilter(s, CHAT_HIDDEN, null)).toBe(true);
    }
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
