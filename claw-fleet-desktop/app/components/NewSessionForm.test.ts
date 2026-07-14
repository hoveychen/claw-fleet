import { describe, expect, it } from "vitest";
import { distinctWorkspaces, isTempWorkspacePath, repoRootPath } from "./NewSessionForm";

describe("isTempWorkspacePath", () => {
  it("flags scratchpad paths under /private/tmp", () => {
    expect(
      isTempWorkspacePath(
        "/private/tmp/claude-501/-Users-hoveychen-workspace-claude-fleet/abc/scratchpad",
      ),
    ).toBe(true);
  });

  it("flags paths under /tmp", () => {
    expect(isTempWorkspacePath("/tmp/claude-501/foo")).toBe(true);
  });

  it("flags macOS per-user temp under /var/folders", () => {
    expect(isTempWorkspacePath("/var/folders/xy/abc123/T/something")).toBe(true);
  });

  it("leaves a real project path alone", () => {
    expect(isTempWorkspacePath("/Users/hoveychen/workspace/claude-fleet")).toBe(false);
  });

  it("does not flag a project whose name merely contains 'tmp'", () => {
    expect(isTempWorkspacePath("/Users/hoveychen/workspace/tmp-tools")).toBe(false);
  });
});

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

  it("drops the chat workspace from recents — it is pinned separately", () => {
    // Without this the launcher would list it twice once chatting has started:
    // once as the pinned entry, once as an ordinary recent workspace.
    const out = distinctWorkspaces(
      [
        { workspacePath: "/Users/foo/.fleet/chat", workspaceName: "Chat", lastActivityMs: 30 },
        { workspacePath: "/w/a", workspaceName: "a", lastActivityMs: 20 },
      ],
      30,
      "/Users/foo/.fleet/chat",
    );
    expect(out.map((w) => w.path)).toEqual(["/w/a"]);
  });

  it("keeps the chat workspace listed when the path is unknown (backend offline)", () => {
    const out = distinctWorkspaces([
      { workspacePath: "/Users/foo/.fleet/chat", workspaceName: "Chat", lastActivityMs: 30 },
    ]);
    expect(out).toHaveLength(1);
  });

  it("drops sessions whose cwd is a temp/scratchpad path", () => {
    const out = distinctWorkspaces([
      {
        workspacePath:
          "/private/tmp/claude-501/-Users-hoveychen-workspace-claude-fleet/abc/scratchpad",
        workspaceName: "scratchpad",
        lastActivityMs: 100,
      },
      { workspacePath: "/Users/hoveychen/workspace/claude-fleet", workspaceName: "claude-fleet", lastActivityMs: 50 },
    ]);
    expect(out.map((w) => w.path)).toEqual(["/Users/hoveychen/workspace/claude-fleet"]);
  });
});
