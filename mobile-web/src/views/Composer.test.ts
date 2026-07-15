import { describe, expect, it } from "vitest";
import { recentWorkspaces } from "./Composer";
import type { SessionInfo } from "../types";

/**
 * 新会话 sheet 提交成功后清空草稿，下次打开退回 recents[0]。桌面端靠「recents 按
 * 最近活动倒序」让刚用过的 workspace 成为默认，从而记住最后一次使用的路径；移动端
 * 曾按名字字母序排，导致默认永远是字母最靠前的那个而非刚用过的那个。
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
  it("按最近活动倒序排列——刚用过的 workspace 排在首位（记住最后一次使用的路径）", () => {
    // Zebra 名字字母最靠后，但活动时间最新；字母序会把它排到最后。
    const sessions = [
      session("/home/alpha", "Alpha", 100),
      session("/home/mid", "Mid", 200),
      session("/home/zebra", "Zebra", 300),
    ];
    const recents = recentWorkspaces(sessions, null);
    expect(recents.map(([path]) => path)).toEqual([
      "/home/zebra",
      "/home/mid",
      "/home/alpha",
    ]);
    // recents[0] 是刚用过（最近活动）的那个——新会话默认落到它上。
    expect(recents[0][0]).toBe("/home/zebra");
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
