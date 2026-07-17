import { describe, expect, it } from "vitest";
import { defaultWorkspace, recentWorkspaces } from "./Composer";
import type { SessionInfo } from "../types";

/**
 * 下拉列表**按名称字母序**排列（方便扫读）；默认选中不再依赖排序，而是来自独立持久化的
 * 「上次成功创建会话用的 repo」（defaultWorkspace）。这解决了旧字母序方案的痛点——那时
 * 默认永远是字母最靠前的那个而非刚用过的那个，如今字母序仅决定展示顺序。
 */
function session(
  workspacePath: string,
  workspaceName: string,
  lastActivityMs: number,
): SessionInfo {
  return {
    id: workspacePath + lastActivityMs,
    workspacePath,
    workspaceName,
    lastActivityMs,
  } as unknown as SessionInfo;
}

describe("recentWorkspaces", () => {
  it("候选未超过 limit 时全部保留，按名称字母序展示", () => {
    // Zebra 活动时间最新，但都在 limit 内，全部保留后只按名字排序。
    const sessions = [
      session("/home/zebra", "Zebra", 300),
      session("/home/alpha", "Alpha", 100),
      session("/home/mid", "Mid", 200),
    ];
    const recents = recentWorkspaces(sessions, null);
    expect(recents.map(([path]) => path)).toEqual([
      "/home/alpha",
      "/home/mid",
      "/home/zebra",
    ]);
  });

  it("候选超过 limit 时先按最近活动截断，再按名称字母序展示（对齐桌面端）", () => {
    // limit=2：Alpha(100) 最久未活跃被丢弃，幸存的 Mid/Zebra 再按名字排序。
    const sessions = [
      session("/home/zebra", "Zebra", 300),
      session("/home/alpha", "Alpha", 100),
      session("/home/mid", "Mid", 200),
    ];
    const recents = recentWorkspaces(sessions, null, 2);
    expect(recents.map(([path]) => path)).toEqual(["/home/mid", "/home/zebra"]);
  });

  it("同一路径多条会话时，取最近活动的时间戳与名字去重", () => {
    const sessions = [
      session("/home/repo", "OldName", 100),
      session("/home/repo", "NewName", 500),
      session("/home/other", "Other", 200),
    ];
    const recents = recentWorkspaces(sessions, null);
    expect(recents).toEqual([
      ["/home/repo", "NewName"],
      ["/home/other", "Other"],
    ]);
  });

  it("剔除纯聊天路径", () => {
    const sessions = [
      session("/home/chat", "Chat", 400),
      session("/home/repo", "Repo", 100),
    ];
    const recents = recentWorkspaces(sessions, "/home/chat");
    expect(recents.map(([path]) => path)).toEqual(["/home/repo"]);
  });
});

describe("defaultWorkspace", () => {
  const recents: [string, string][] = [
    ["/home/alpha", "Alpha"],
    ["/home/mango", "Mango"],
    ["/home/zebra", "Zebra"],
  ];

  it("沿用用户本次已选且有效的 workspace", () => {
    expect(defaultWorkspace("/home/zebra", recents, null, "/home/alpha")).toBe("/home/zebra");
  });

  it("沿用 __custom__（自定义路径）选择", () => {
    expect(defaultWorkspace("__custom__", recents, null, "/home/alpha")).toBe("__custom__");
  });

  it("草稿为空时默认选中上次用过的 repo", () => {
    expect(defaultWorkspace("", recents, null, "/home/mango")).toBe("/home/mango");
  });

  it("上次用过的 repo 已失效时退回列表首项（字母序）", () => {
    expect(defaultWorkspace("", recents, null, "/home/deleted")).toBe("/home/alpha");
  });

  it("无记忆、无候选时退回纯聊天路径", () => {
    expect(defaultWorkspace("", [], "/home/chat", "")).toBe("/home/chat");
  });

  it("纯聊天路径可作为上次用过的目标被记住", () => {
    expect(defaultWorkspace("", recents, "/home/chat", "/home/chat")).toBe("/home/chat");
  });
});
