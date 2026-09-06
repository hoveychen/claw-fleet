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

  it("uses the directory name as a deterministic tie-breaker", () => {
    const groups = groupSessionsByWorkspace([
      session("z", "/work/zebra", "zebra", 50),
      session("a", "/work/apple", "apple", 50),
    ]);

    expect(groups.map((group) => group.name)).toEqual(["apple", "zebra"]);
  });
});
