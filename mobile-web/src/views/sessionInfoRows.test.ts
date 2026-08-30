import { describe, expect, it } from "vitest";
import { buildInfoRows, resumeCommand } from "./sessionInfoRows";
import type { SessionInfo } from "../types";

// label 随语言变，所以断言一律走 key / value。
const base: SessionInfo = {
  id: "abc-123",
  workspacePath: "/Users/x/workspace/proj",
  workspaceName: "proj",
  status: "idle",
  isSubagent: false,
  lastActivityMs: 1_700_000_000_000,
  createdAtMs: 1_699_990_000_000,
  jsonlPath: "/Users/x/.claude/projects/proj/abc-123.jsonl",
};

const keys = (s: SessionInfo) => buildInfoRows(s).map((r) => r.key);
const valueOf = (s: SessionInfo, key: string) =>
  buildInfoRows(s).find((r) => r.key === key)?.value;

describe("buildInfoRows", () => {
  it("给出会话 ID 与工作区名", () => {
    expect(valueOf(base, "sessionId")).toBe("abc-123");
    expect(valueOf(base, "workspace")).toBe("proj");
  });

  it("路径与时间不进面板：路径在 ☰ 的复制条目上，时间在每条消息旁", () => {
    const k = keys(base);
    expect(k).not.toContain("workspacePath");
    expect(k).not.toContain("jsonlPath");
    expect(k).not.toContain("created");
    expect(k).not.toContain("lastActivity");
  });

  it("状态/接力/PID 这些在别处已有更好的呈现，不重复搬进来", () => {
    const rich: SessionInfo = {
      ...base,
      pid: 42,
      entrypoint: "claw-fleet-newsession",
      slug: "fix/auth",
      runningSubagentCount: 3,
      handoff: { chainId: "c1", hop: 2, chainLen: 3 },
      watches: [{ id: "w1", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 1 }],
    };
    expect(keys(rich).sort()).toEqual(["sessionId", "workspace"].sort());
  });

  it("缺席的字段不占行（不显示成空值）", () => {
    expect(keys(base)).not.toContain("model");
    expect(valueOf({ ...base, model: "claude-opus-5" }, "model")).toBe("claude-opus-5");
  });

  it("contextPercent 按 0–1 比值换算成百分比", () => {
    expect(valueOf({ ...base, contextPercent: 0.72 }, "context")).toBe("72%");
  });

  it("半分钱以下的花费不成行", () => {
    expect(valueOf({ ...base, totalCostUsd: 0.001 }, "cost")).toBeUndefined();
    expect(valueOf({ ...base, totalCostUsd: 4.331 }, "cost")).toBe("$4.33");
  });
});

describe("resumeCommand", () => {
  it("只给 Claude 会话；codex / dsh 不给可能贴上去就报错的命令", () => {
    expect(resumeCommand(base)).toBe("claude --resume abc-123");
    expect(resumeCommand({ ...base, agentSource: "codex" })).toBeNull();
    expect(resumeCommand({ ...base, agentSource: "dsh" })).toBeNull();
  });
});
