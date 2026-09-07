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

  it("orders folders alphabetically and their rows by latest activity", () => {
    const groups = groupSessionsByWorkspace([
      session("only-z", "/work/zebra", "zebra", 900),
      session("older-a", "/work/apple", "apple", 10),
      session("newer-a", "/work/apple", "apple", 40),
    ]);

    // `zebra` is by far the busiest folder and still sits second: folder order
    // is a stable directory listing, activity only orders rows within a folder.
    expect(groups.map((group) => group.path)).toEqual([
      "/work/apple",
      "/work/zebra",
    ]);
    expect(groups[0].sessions.map((item) => item.id)).toEqual([
      "newer-a",
      "older-a",
    ]);
  });

  it("keeps the caller's row order when told to preserve it, folders still alphabetical", () => {
    // The rail freezes row order while the pointer is parked over it, then
    // hands the frozen list here. Re-sorting by activity would undo the freeze
    // inside a repository section. Folder order is alphabetical regardless —
    // `zebra` leads the input and still sorts last.
    const groups = groupSessionsByWorkspace(
      [
        session("only-z", "/work/zebra", "zebra", 30),
        session("older-a", "/work/apple", "apple", 10),
        session("newer-a", "/work/apple", "apple", 40),
      ],
      { preserveOrder: true },
    );

    expect(groups.map((group) => group.path)).toEqual([
      "/work/apple",
      "/work/zebra",
    ]);
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

    expect(groups.map((group) => group.path)).toEqual(["/work/a", "/work/b"]);
  });

  it("falls back to the path when two folders share a display name", () => {
    const groups = groupSessionsByWorkspace([
      session("z", "/work/z/manta", "manta", 50),
      session("a", "/work/a/manta", "manta", 900),
    ]);

    expect(groups.map((group) => group.path)).toEqual([
      "/work/a/manta",
      "/work/z/manta",
    ]);
  });
});
