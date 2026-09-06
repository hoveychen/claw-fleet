import { describe, expect, it } from "vitest";
import {
  CODEX_MODEL_CHOICES,
  carryPromptToDevice,
  defaultWorkspace,
  recentWorkspaces,
} from "./Composer";
import { loadDraft, saveDraft, type DraftStorage } from "../draft";
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

describe("Codex model choices", () => {
  it("includes GPT-6 Astra", () => {
    expect(CODEX_MODEL_CHOICES).toContainEqual(["gpt-6-astra", "GPT-6 Astra"]);
  });
});

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

  it("worktree checkout 折叠回 repo 根，去重后只出现一次（对齐桌面端）", () => {
    // 同一 repo 的主 checkout 与 .worktrees/<id> 子目录应折叠到 repo 根 /home/repo。
    const sessions = [
      session("/home/repo", "Repo", 100),
      session("/home/repo/.worktrees/feat-x", "Repo", 500),
    ];
    const recents = recentWorkspaces(sessions, null);
    expect(recents).toEqual([["/home/repo", "Repo"]]);
  });

  it("剔除临时目录下的 workspace（/tmp、/private/tmp、/var/folders、/private/var/folders）", () => {
    // macOS 上 /var 软链到 /private/var，规范化后的 cwd 会呈现为 /private/var/folders/...
    const sessions = [
      session("/tmp/scratch", "Scratch", 500),
      session("/private/tmp/foo", "Foo", 400),
      session("/var/folders/ab/T/bar", "Bar", 300),
      session("/private/var/folders/3_/hh7x/T/fleet-codex-e2e-1", "Codex", 200),
      session("/home/repo", "Repo", 100),
    ];
    const recents = recentWorkspaces(sessions, null);
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

// 换新会话目标设备时,只有 prompt 该跟着走。其余每一项(workspace / 模型 /
// 附件路径)都属于某一台具体机器,搬过去就是一串在目标机上不存在的东西。
describe("carryPromptToDevice", () => {
  function memStore(): DraftStorage & { map: Map<string, string> } {
    const map = new Map<string, string>();
    return {
      map,
      getItem: (k) => (map.has(k) ? map.get(k)! : null),
      setItem: (k, v) => void map.set(k, v),
      removeItem: (k) => void map.delete(k),
    };
  }
  const read = (store: DraftStorage, id: string) =>
    loadDraft<Record<string, string>>(`d/${id}/new-session`, {}, store);

  it("prompt 落进目标设备的命名空间,不动来源那台", () => {
    const store = memStore();
    saveDraft("d/mac/new-session", { workspace: "/repos/mac", prompt: "旧的" }, store);
    carryPromptToDevice("cloud", "刚敲的字", store);
    expect(read(store, "cloud").prompt).toBe("刚敲的字");
    // 来源那台的草稿原封不动 —— 切回去应当还是它自己那份。
    expect(read(store, "mac")).toEqual({ workspace: "/repos/mac", prompt: "旧的" });
  });

  it("保留目标设备自己的 workspace / 模型,只覆盖 prompt", () => {
    const store = memStore();
    saveDraft(
      "d/cloud/new-session",
      { workspace: "/workspace/repo", model: "sonnet", prompt: "云端上没提交的" },
      store,
    );
    carryPromptToDevice("cloud", "换过来的字", store);
    expect(read(store, "cloud")).toMatchObject({
      workspace: "/workspace/repo",
      model: "sonnet",
      prompt: "换过来的字",
    });
  });

  it("目标设备还没有草稿时,拿到默认值 + 这段 prompt", () => {
    const store = memStore();
    carryPromptToDevice("fresh", "第一句", store);
    const d = read(store, "fresh");
    expect(d.prompt).toBe("第一句");
    // 默认值必须在(不是只存了个 {prompt}),否则重挂载后 tool/permissionMode 会是 undefined。
    expect(d.tool).toBe("claude");
    expect(d.permissionMode).toBe("acceptEdits");
  });
});
