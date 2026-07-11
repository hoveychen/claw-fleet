import { describe, expect, it } from "vitest";
import { distinctWorkspaces, repoRootPath } from "./NewSessionForm";

describe("repoRootPath", () => {
  it("collapses an in-repo worktree checkout to its repo root", () => {
    expect(
      repoRootPath("/Users/hoveychen/workspace/maliang/.worktrees/script-runtime"),
    ).toBe("/Users/hoveychen/workspace/maliang");
  });

  it("collapses regardless of the task-id leaf, incl. trailing slash", () => {
    expect(repoRootPath("/Users/foo/my-repo/.worktrees/fix-bug/")).toBe(
      "/Users/foo/my-repo",
    );
  });

  it("leaves a plain repo path unchanged", () => {
    expect(repoRootPath("/Users/hoveychen/workspace/maliang")).toBe(
      "/Users/hoveychen/workspace/maliang",
    );
  });

  it("leaves ~/.fleet/worktrees task-workers unchanged (segment is 'worktrees', not '.worktrees')", () => {
    expect(repoRootPath("/Users/hoveychen/.fleet/worktrees/task-x/p1")).toBe(
      "/Users/hoveychen/.fleet/worktrees/task-x/p1",
    );
  });

  it("does not collapse a leading .worktrees segment (idx 0)", () => {
    expect(repoRootPath("/.worktrees/x")).toBe("/.worktrees/x");
  });
});

describe("distinctWorkspaces", () => {
  it("collapses a repo's main checkout and its worktree into one entry at the root", () => {
    // The reported bug: the launcher listed both `maliang` and
    // `maliang/.worktrees/...` as separate options.
    const out = distinctWorkspaces([
      {
        workspacePath: "/Users/hoveychen/workspace/maliang/.worktrees/script-runtime",
        workspaceName: "maliang",
        lastActivityMs: 200,
      },
      {
        workspacePath: "/Users/hoveychen/workspace/maliang",
        workspaceName: "maliang",
        lastActivityMs: 100,
      },
    ]);
    expect(out).toHaveLength(1);
    expect(out[0].path).toBe("/Users/hoveychen/workspace/maliang");
  });

  it("offers the repo root even when only a worktree session exists", () => {
    const out = distinctWorkspaces([
      {
        workspacePath: "/Users/hoveychen/workspace/maliang/.worktrees/fix-bug",
        workspaceName: "maliang",
        lastActivityMs: 50,
      },
    ]);
    expect(out).toHaveLength(1);
    expect(out[0].path).toBe("/Users/hoveychen/workspace/maliang");
    expect(out[0].name).toBe("maliang");
  });

  it("keeps distinct repos separate and sorts most-recent first", () => {
    const out = distinctWorkspaces([
      { workspacePath: "/w/a", workspaceName: "a", lastActivityMs: 10 },
      { workspacePath: "/w/b", workspaceName: "b", lastActivityMs: 20 },
    ]);
    expect(out.map((w) => w.name)).toEqual(["b", "a"]);
  });

  it("skips sessions without a workspacePath", () => {
    const out = distinctWorkspaces([
      { workspacePath: null, workspaceName: null, lastActivityMs: 10 },
      { workspacePath: "/w/a", workspaceName: "a", lastActivityMs: 20 },
    ]);
    expect(out).toHaveLength(1);
  });
});
