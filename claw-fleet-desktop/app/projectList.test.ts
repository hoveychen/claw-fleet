import { describe, expect, it } from "vitest";
import {
  archivedPaths,
  archivedProjects,
  mergeProjects,
  pathTail,
  withBrowsePaths,
  type ProjectSessionView,
} from "./projectList";

const session = (over: Partial<ProjectSessionView>): ProjectSessionView => ({
  workspacePath: "/u/repo",
  workspaceName: "repo",
  isSubagent: false,
  procAlive: false,
  lastActivityMs: 0,
  ...over,
});

describe("mergeProjects", () => {
  it("unions the registry with session repo roots, folding worktrees", () => {
    const rows = mergeProjects(
      [{ path: "/u/a", name: "a", createdAtMs: 5 }],
      [
        session({ workspacePath: "/u/b/.worktrees/x", workspaceName: "x", lastActivityMs: 10, procAlive: true }),
        session({ workspacePath: "/u/b", workspaceName: "b", lastActivityMs: 3, isSubagent: true, procAlive: true }),
      ],
      null,
    );
    expect(rows).toEqual([
      { path: "/u/b", name: "b", lastActiveMs: 10, runningTasks: 1 },
      { path: "/u/a", name: "a", lastActiveMs: 5, runningTasks: 0 },
    ]);
  });

  it("drops temp dirs and the chat workspace", () => {
    const rows = mergeProjects(
      [{ path: "/tmp/scratch", name: "scratch", createdAtMs: 1 }],
      [
        session({ workspacePath: "/private/var/folders/x/y" }),
        session({ workspacePath: "/u/.fleet/chat" }),
      ],
      "/u/.fleet/chat",
    );
    expect(rows).toEqual([]);
  });
});

describe("withBrowsePaths", () => {
  const row = { path: "/w/a", name: "a", lastActiveMs: 5, runningTasks: 0 };

  it("puts hand-added directories first and marks them removable", () => {
    const out = withBrowsePaths([row], ["/w/new/", "/w/a"]);
    expect(out.map((r) => r.path)).toEqual(["/w/new", "/w/a"]);
    expect(out[0]).toMatchObject({ name: "new", handAdded: true });
    expect(out[1].handAdded).toBeUndefined();
  });

  it("keeps a hand-added temp directory, which mergeProjects would drop", () => {
    expect(withBrowsePaths([], ["/tmp/scratch"]).map((r) => r.path)).toEqual(["/tmp/scratch"]);
  });
});

describe("archived projects", () => {
  const registered = [
    { path: "/u/live", name: "live", createdAtMs: 1 },
    { path: "/u/old", name: "old", createdAtMs: 1, archivedAtMs: 20 },
    { path: "/u/older/", name: "", createdAtMs: 1, archivedAtMs: 10 },
  ];

  it("leaves archived projects out of the list, sessions in them included", () => {
    const rows = mergeProjects(
      registered,
      [session({ workspacePath: "/u/old/.worktrees/t", workspaceName: "t", lastActivityMs: 99, procAlive: true })],
      null,
    );
    expect(rows.map((r) => r.path)).toEqual(["/u/live"]);
  });

  it("does not bring an archived hand-added directory back", () => {
    const rows = withBrowsePaths([], ["/u/old", "/u/new"], archivedPaths(registered));
    expect(rows.map((r) => r.path)).toEqual(["/u/new"]);
  });

  it("lists archived projects newest first, names filled in", () => {
    expect(archivedProjects(registered).map((p) => [p.path, p.name])).toEqual([
      ["/u/old", "old"],
      ["/u/older", "older"],
    ]);
  });
});

describe("pathTail", () => {
  it("keeps the last two segments", () => {
    expect(pathTail("/Users/me/work/repo")).toBe("…/work/repo");
    expect(pathTail("/repo")).toBe("/repo");
  });
});
