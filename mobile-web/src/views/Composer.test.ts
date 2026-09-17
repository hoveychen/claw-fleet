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
  resumeConfigOverrides,
} from "./Composer";
import { effortChoicesFor, modelChoicesFor } from "../useModelCatalog";
import type { PickerHarness } from "../generated/types";
import { loadDraft, saveDraft, type DraftStorage } from "../draft";
import type { SessionInfo } from "../types";

/**
 * Dropdown list is sorted **alphabetically by name** (easy to scan); default
 * selection no longer depends on sort order, but comes from independently
 * persisted "last repo used to create a session" (defaultWorkspace). This fixes
 * the old alphabetic-sort pain point — the default was always the first letter
 * instead of the most recently used, now alphabetic order only affects display.
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

describe("Model / effort dropdown (from model_catalog)", () => {
  // Shape matches actual `model_catalog` return. Effort ladder intentionally
  // differs per model — that's exactly where the old hand-maintained lists got
  // it wrong (claiming Codex only goes to high and has minimal).
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

  it("dropdown starts with 'default', then models from catalog", () => {
    expect(modelChoicesFor(catalog, "codex", "默认模型")).toEqual([
      ["", "默认模型"],
      ["gpt-6-astra", "GPT-6 Astra"],
      ["gpt-5.5", "GPT-5.5"],
    ]);
  });

  it("when catalog is empty, only 'default' remains", () => {
    expect(modelChoicesFor([], "codex", "默认模型")).toEqual([["", "默认模型"]]);
  });

  it("effort follows the chosen model, not the entire harness", () => {
    expect(effortChoicesFor(catalog, "codex", "gpt-5.5", "默认").map(([v]) => v)).toEqual([
      "",
      "low",
      "medium",
      "high",
      "xhigh",
    ]);
    const astra = effortChoicesFor(catalog, "codex", "gpt-6-astra", "默认").map(([v]) => v);
    expect(astra).toContain("ultra");
    // Old list invented minimal out of nowhere; no Codex model accepts it.
    expect(astra).not.toContain("minimal");
  });

  it("when no model chosen, return union of efforts across harness", () => {
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
  it("when candidates don't exceed limit, keep all and sort by name alphabetically", () => {
    // Zebra is most recently active, but all within limit, so keep all and sort
    // by name only.
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

  it("when candidates exceed limit, truncate by recent activity then sort by name (align with desktop)", () => {
    // limit=2: Alpha(100) least recently active is discarded, survivors Mid/Zebra
    // then sorted by name.
    const sessions = [
      session("/home/zebra", "Zebra", 300),
      session("/home/alpha", "Alpha", 100),
      session("/home/mid", "Mid", 200),
    ];
    const recents = recentWorkspaces(sessions, null, 2);
    expect(recents.map(([path]) => path)).toEqual(["/home/mid", "/home/zebra"]);
  });

  it("for same path with multiple sessions, deduplicate by most recent timestamp and name", () => {
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

  it("exclude pure chat path", () => {
    const sessions = [
      session("/home/chat", "Chat", 400),
      session("/home/repo", "Repo", 100),
    ];
    const recents = recentWorkspaces(sessions, "/home/chat");
    expect(recents.map(([path]) => path)).toEqual(["/home/repo"]);
  });

  it("worktree checkout collapses to repo root, appears once after dedup (align with desktop)", () => {
    // Main checkout and .worktrees/<id> subdirectory of the same repo should
    // collapse to repo root /home/repo.
    const sessions = [
      session("/home/repo", "Repo", 100),
      session("/home/repo/.worktrees/feat-x", "Repo", 500),
    ];
    const recents = recentWorkspaces(sessions, null);
    expect(recents).toEqual([["/home/repo", "Repo"]]);
  });

  it("exclude workspaces in temp dirs (/tmp, /private/tmp, /var/folders, /private/var/folders)", () => {
    // On macOS /var is symlink to /private/var, so normalized cwd appears as
    // /private/var/folders/...
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

  it("keep using the workspace user selected this session if valid", () => {
    expect(defaultWorkspace("/home/zebra", recents, null, "/home/alpha")).toBe("/home/zebra");
  });

  it("keep using __custom__ (custom path) selection", () => {
    expect(defaultWorkspace("__custom__", recents, null, "/home/alpha")).toBe("__custom__");
  });

  it("when draft is empty, default to the last used repo", () => {
    expect(defaultWorkspace("", recents, null, "/home/mango")).toBe("/home/mango");
  });

  it("when last used repo is invalid, fall back to list first item (alphabetically)", () => {
    expect(defaultWorkspace("", recents, null, "/home/deleted")).toBe("/home/alpha");
  });

  it("with no memory and no candidates, fall back to pure chat path", () => {
    expect(defaultWorkspace("", [], "/home/chat", "")).toBe("/home/chat");
  });

  it("pure chat path can be remembered as last used target", () => {
    expect(defaultWorkspace("", recents, "/home/chat", "/home/chat")).toBe("/home/chat");
  });
});

describe("new-session summaries", () => {
  it("compress device and project into a scannable location summary", () => {
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

  it("pure chat summary does not leak original workspace", () => {
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

  it("compress Agent, model, effort, and permission into config summary", () => {
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

  it("default model and effort still show explicitly, no blank summary", () => {
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

// When switching target device for a new session, only prompt should move with it.
// Everything else (workspace / model / attachment path) belongs to a specific
// machine; moving it would be a bunch of nonexistent paths on the target.
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

  it("prompt lands in target device's namespace, source device unchanged", () => {
    const store = memStore();
    saveDraft("d/mac/new-session", { workspace: "/repos/mac", prompt: "旧的" }, store);
    carryPromptToDevice("cloud", "刚敲的字", store);
    expect(read(store, "cloud").prompt).toBe("刚敲的字");
    // Source device's draft untouched — switching back should still have its own.
    expect(read(store, "mac")).toEqual({ workspace: "/repos/mac", prompt: "旧的" });
  });

  it("preserve target device's own workspace / model, only override prompt", () => {
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

  it("when target device has no draft, get defaults + this prompt", () => {
    const store = memStore();
    carryPromptToDevice("fresh", "第一句", store);
    const d = read(store, "fresh");
    expect(d.prompt).toBe("第一句");
    // Defaults must be present (not just {prompt}), else after remount tool/
    // permissionMode would be undefined.
    expect(d.tool).toBe("claude");
    expect(d.permissionMode).toBe("acceptEdits");
  });
});

describe("resumeConfigChips", () => {
  const labels = { defaultModel: "默认模型", defaultPermission: "沿用权限" };

  it("model and effort combine into one chip, permission is separate", () => {
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

  it("when unselected, report defaults instead of empty chip", () => {
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

  it("codex / dsh don't show permission chip — they have no --permission-mode concept", () => {
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

  it("running sessions under same project are counted, timestamp from most recent", () => {
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

  it("after worktree collapses to repo root, running sessions from both sides merge into count", () => {
    const rows = recentWorkspaceRows(
      [
        live("/home/repo", "Repo", 100, "executing"),
        live("/home/repo/.worktrees/feat-x", "Repo", 900, "streaming"),
      ],
      null,
    );
    expect(rows).toEqual([{ path: "/home/repo", name: "Repo", lastMs: 900, running: 2 }]);
  });

  it("when all idle, running is 0", () => {
    const rows = recentWorkspaceRows([live("/home/repo", "Repo", 100, "idle")], null);
    expect(rows[0].running).toBe(0);
  });
});

describe("composerInset", () => {
  it("layout height plus bottom offset equals space transcript must leave", () => {
    expect(composerInset(196, "22px")).toBe(218);
  });

  it("when decision collapse bar pushes chip higher, space to clear grows", () => {
    // After --peek-inset takes effect, computed bottom rises from 22px to 78px.
    expect(composerInset(196, "78px")).toBe(274);
  });

  it("when bottom can't be parsed (auto), use only own height, never return NaN", () => {
    expect(composerInset(196, "auto")).toBe(196);
  });

  it("input is layout value, transform doesn't enter — subpixel converges to integer", () => {
    // This locks the measurement path: switching back to getBoundingClientRect
    // would be polluted by transform.
    expect(composerInset(196.4, "21.6px")).toBe(218);
  });
});

describe("resumeConfigOverrides", () => {
  // Real incident 2026-09-13: desktop starts codex session with gpt-6-astra, user
  // asks from phone, relay gets model=None → `codex exec resume` without -m →
  // codex falls back to gpt-5.6-sol in ~/.codex/config.toml, entire thread
  // switches models.
  it("when user hasn't touched config, send no fields — let desktop get authoritative values from launch-spec", () => {
    // Initial value from snapshot (session currently runs on astra) but not
    // manually changed.
    expect(resumeConfigOverrides({ touched: false, model: "gpt-6-astra", effort: "high" })).toEqual(
      {},
    );
  });

  it("when manually changed, send as is — user explicitly switching models mid-thread", () => {
    expect(resumeConfigOverrides({ touched: true, model: "gpt-5.6-sol", effort: "" })).toEqual({
      model: "gpt-5.6-sol",
    });
  });

  it("when changed to 'default' (empty string), don't send field — don't treat empty string as a model id", () => {
    expect(resumeConfigOverrides({ touched: true, model: "", effort: "medium" })).toEqual({
      effort: "medium",
    });
  });
});
