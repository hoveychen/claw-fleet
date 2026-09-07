import { describe, expect, it } from "vitest";
import type { SessionInfo } from "../types";
import { groupSessionsByWorkspace } from "./workspaceSessionGroups";

function session(
  id: string,
  workspacePath: string,
  workspaceName: string,
  agentLastActivityMs: number,
): SessionInfo {
  return {
    id,
    jsonlPath: `/sessions/${id}.jsonl`,
    workspacePath,
    workspaceName,
    lastActivityMs: agentLastActivityMs,
    agentLastActivityMs,
  } as SessionInfo;
}

describe("groupSessionsByWorkspace", () => {
  it("folds worktree sessions into their durable repository root", () => {
    const groups = groupSessionsByWorkspace([
      session("main", "/work/manta", "manta", 100),
      session("branch", "/work/manta/.worktrees/fix-sidebar", "manta", 200),
    ]);

    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({
      path: "/work/manta",
      name: "manta",
      latestActivityMs: 200,
    });
    expect(groups[0].sessions.map((item) => item.id)).toEqual(["branch", "main"]);
  });

  it("groups distinct directories and orders groups by their latest activity", () => {
    const groups = groupSessionsByWorkspace([
      session("older-a", "/work/a", "a", 10),
      session("newer-a", "/work/a", "a", 40),
      session("only-b", "/work/b", "b", 30),
    ]);

    expect(groups.map((group) => group.path)).toEqual(["/work/a", "/work/b"]);
    expect(groups[0].sessions.map((item) => item.id)).toEqual(["newer-a", "older-a"]);
  });

  it("keeps the caller's order when told to preserve it", () => {
    // The rail freezes row order while the pointer is parked over it, then
    // hands the frozen list here. Re-sorting by activity would undo the freeze
    // both inside a repository section and across sections.
    const groups = groupSessionsByWorkspace(
      [
        session("older-a", "/work/a", "a", 10),
        session("only-b", "/work/b", "b", 30),
        session("newer-a", "/work/a", "a", 40),
      ],
      { preserveOrder: true },
    );

    expect(groups.map((group) => group.path)).toEqual(["/work/a", "/work/b"]);
    expect(groups[0].sessions.map((item) => item.id)).toEqual([
      "older-a",
      "newer-a",
    ]);
  });

  it("pins the given path to the top however stale it is", () => {
    const groups = groupSessionsByWorkspace(
      [
        session("busy", "/work/repo", "repo", 900),
        session("quiet", "/home/me/.fleet/chat", "Chat", 1),
      ],
      { pinnedPath: "/home/me/.fleet/chat" },
    );

    expect(groups.map((group) => group.path)).toEqual([
      "/home/me/.fleet/chat",
      "/work/repo",
    ]);
  });

  it("pins under preserveOrder too, leaving the rest of the order intact", () => {
    const groups = groupSessionsByWorkspace(
      [
        session("a", "/work/a", "a", 10),
        session("b", "/work/b", "b", 30),
        session("chat", "/home/me/.fleet/chat", "Chat", 1),
      ],
      { preserveOrder: true, pinnedPath: "/home/me/.fleet/chat" },
    );

    expect(groups.map((group) => group.path)).toEqual([
      "/home/me/.fleet/chat",
      "/work/a",
      "/work/b",
    ]);
  });

  it("ignores a pinned path with no section of its own", () => {
    const groups = groupSessionsByWorkspace(
      [session("a", "/work/a", "a", 10), session("b", "/work/b", "b", 30)],
      { pinnedPath: "/home/me/.fleet/chat" },
    );

    expect(groups.map((group) => group.path)).toEqual(["/work/b", "/work/a"]);
  });

  it("uses the directory name as a deterministic tie-breaker", () => {
    const groups = groupSessionsByWorkspace([
      session("z", "/work/zebra", "zebra", 50),
      session("a", "/work/apple", "apple", 50),
    ]);

    expect(groups.map((group) => group.name)).toEqual(["apple", "zebra"]);
  });
});
