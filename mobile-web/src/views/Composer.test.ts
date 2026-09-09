import { describe, expect, it } from "vitest";
import {
  carryPromptToDevice,
  composerInset,
  defaultWorkspace,
  newSessionConfigSummary,
  newSessionLocationSummary,
  recentWorkspaces,
  recentWorkspaceRows,
  resumeConfigChips,
} from "./Composer";
import { effortChoicesFor, modelChoicesFor } from "../useModelCatalog";
import type { PickerHarness } from "../generated/types";
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

describe("模型 / 努力度下拉（来自 model_catalog）", () => {
  // 形状与 `model_catalog` 真实返回一致。梯子刻意逐模型不同——那正是旧的两份
  // 手抄清单写错的地方（它们声称 Codex 只到 high 且有 minimal）。
  const catalog: PickerHarness[] = [
    {
      name: "codex",
      available: true,
      models: [
        {
          id: "gpt-6-astra",
          label: "GPT-6 Astra",
          harness: "codex",
          tier: "premium",
          efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
          defaultEffort: "medium",
        },
        {
          id: "gpt-5.5",
          label: "GPT-5.5",
          harness: "codex",
          tier: "premium",
          efforts: ["low", "medium", "high", "xhigh"],
          defaultEffort: "xhigh",
        },
      ],
    },
  ];

  it("下拉开头是「默认」，其后是目录里的模型", () => {
    expect(modelChoicesFor(catalog, "codex", "默认模型")).toEqual([
      ["", "默认模型"],
      ["gpt-6-astra", "GPT-6 Astra"],
      ["gpt-5.5", "GPT-5.5"],
    ]);
  });

  it("目录没到时只剩「默认」", () => {
    expect(modelChoicesFor([], "codex", "默认模型")).toEqual([["", "默认模型"]]);
  });

  it("努力度跟着选中的那个模型走，而不是整个 harness", () => {
    expect(effortChoicesFor(catalog, "codex", "gpt-5.5", "默认").map(([v]) => v)).toEqual([
      "",
      "low",
      "medium",
      "high",
      "xhigh",
    ]);
    const astra = effortChoicesFor(catalog, "codex", "gpt-6-astra", "默认").map(([v]) => v);
    expect(astra).toContain("ultra");
    // 旧清单凭空发明了 minimal；没有任何 Codex 模型接受它。
    expect(astra).not.toContain("minimal");
  });

  it("没选模型时给该 harness 内的并集", () => {
    expect(effortChoicesFor(catalog, "codex", "", "默认").map(([v]) => v)).toEqual([
      "",
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
      "ultra",
    ]);
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

describe("new-session summaries", () => {
  it("把设备与项目压成一条可扫读的位置摘要", () => {
    expect(
      newSessionLocationSummary({
        deviceLabel: "Mac Studio",
        connected: true,
        isChat: false,
        workspaceName: "api-server",
        workspacePath: "/Users/demo/workspace/api-server",
      }),
    ).toEqual({
      title: "Mac Studio · api-server",
      detail: "在线 · /Users/demo/workspace/api-server",
    });
  });

  it("纯聊天摘要不泄漏原 workspace", () => {
    expect(
      newSessionLocationSummary({
        deviceLabel: "Mac Studio",
        connected: false,
        isChat: true,
        workspaceName: "api-server",
        workspacePath: "/Users/demo/workspace/api-server",
      }),
    ).toEqual({
      title: "Mac Studio · 纯聊天",
      detail: "离线 · 不绑定任何项目目录",
    });
  });

  it("把 Agent、模型、effort 与权限压成配置摘要", () => {
    expect(
      newSessionConfigSummary({
        toolLabel: "Claude",
        modelLabel: "Opus 5",
        effortLabel: "xhigh",
        permissionLabel: "自动接受编辑",
      }),
    ).toEqual({
      title: "Claude · Opus 5 · xhigh",
      detail: "自动接受编辑",
    });
  });

  it("默认模型与 effort 仍明确显示，不留空白摘要", () => {
    expect(
      newSessionConfigSummary({
        toolLabel: "Codex",
        modelLabel: "",
        effortLabel: "",
        permissionLabel: "",
      }),
    ).toEqual({
      title: "Codex · 默认模型 · 默认努力度",
      detail: "按 Agent 默认权限运行",
    });
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

describe("resumeConfigChips", () => {
  const labels = { defaultModel: "默认模型", defaultPermission: "沿用权限" };

  it("模型与档位合成一颗，权限单独一颗", () => {
    expect(
      resumeConfigChips({
        tool: "claude",
        modelLabel: "Opus 5",
        effortLabel: "xhigh",
        permissionLabel: "接受编辑",
        labels,
      }),
    ).toEqual(["Opus 5 · xhigh", "接受编辑"]);
  });

  it("没选就报告默认值，而不是空胶囊", () => {
    expect(
      resumeConfigChips({
        tool: "claude",
        modelLabel: "",
        effortLabel: "",
        permissionLabel: "",
        labels,
      }),
    ).toEqual(["默认模型", "沿用权限"]);
  });

  it("codex / dsh 不出权限胶囊——它们没有 --permission-mode 这个概念", () => {
    for (const tool of ["codex", "dsh"]) {
      expect(
        resumeConfigChips({
          tool,
          modelLabel: "gpt-5.6-sol",
          effortLabel: "medium",
          permissionLabel: "接受编辑",
          labels,
        }),
      ).toEqual(["gpt-5.6-sol · medium"]);
    }
  });
});

describe("recentWorkspaceRows", () => {
  function live(path: string, name: string, ms: number, status: string): SessionInfo {
    return { ...session(path, name, ms), status } as unknown as SessionInfo;
  }

  it("同一项目下的在跑会话被数出来，时间戳取最近的那条", () => {
    const rows = recentWorkspaceRows(
      [
        live("/home/repo", "Repo", 100, "executing"),
        live("/home/repo", "Repo", 500, "idle"),
        live("/home/repo", "Repo", 300, "thinking"),
      ],
      null,
    );
    expect(rows).toEqual([{ path: "/home/repo", name: "Repo", lastMs: 500, running: 2 }]);
  });

  it("worktree 折叠进 repo 根之后，两边的在跑会话合并计数", () => {
    const rows = recentWorkspaceRows(
      [
        live("/home/repo", "Repo", 100, "executing"),
        live("/home/repo/.worktrees/feat-x", "Repo", 900, "streaming"),
      ],
      null,
    );
    expect(rows).toEqual([{ path: "/home/repo", name: "Repo", lastMs: 900, running: 2 }]);
  });

  it("全是闲置时 running 为 0", () => {
    const rows = recentWorkspaceRows([live("/home/repo", "Repo", 100, "idle")], null);
    expect(rows[0].running).toBe(0);
  });
});

describe("composerInset", () => {
  it("布局高度加上 bottom 偏移，就是转录区要让开的那一截", () => {
    expect(composerInset(196, "22px")).toBe(218);
  });

  it("决策折叠条把胶囊顶高时，让开的距离跟着变大", () => {
    // --peek-inset 生效后 computed bottom 从 22px 涨到 78px。
    expect(composerInset(196, "78px")).toBe(274);
  });

  it("bottom 解析不出来（auto）时只算自身高度，绝不报 NaN", () => {
    expect(composerInset(196, "auto")).toBe(196);
  });

  it("入参是布局值，transform 进不来 —— 亚像素也按整数收敛", () => {
    // 这条锁住的是取值口径：换回 getBoundingClientRect 就会被 transform 污染。
    expect(composerInset(196.4, "21.6px")).toBe(218);
  });
});
