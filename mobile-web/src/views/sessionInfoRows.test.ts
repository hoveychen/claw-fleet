import { describe, expect, it } from "vitest";
import { buildInfoRows, resumeCommand, sourceLabel } from "./sessionInfoRows";
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

const keys = (s: SessionInfo) => buildInfoRows(s, 1_700_000_060_000).map((r) => r.key);
const valueOf = (s: SessionInfo, key: string) =>
  buildInfoRows(s, 1_700_000_060_000).find((r) => r.key === key)?.value;

describe("buildInfoRows", () => {
  it("总是给出身份三件套：会话 ID、工作区路径、会话记录", () => {
    expect(valueOf(base, "sessionId")).toBe("abc-123");
    expect(valueOf(base, "workspacePath")).toBe("/Users/x/workspace/proj");
    expect(valueOf(base, "jsonlPath")).toBe(base.jsonlPath);
  });

  it("缺席的字段不占行（不显示成空值）", () => {
    const k = keys(base);
    expect(k).not.toContain("model");
    expect(k).not.toContain("context");
    expect(k).not.toContain("cost");
    expect(k).not.toContain("pid");
    expect(k).not.toContain("handoff");
    expect(k).not.toContain("watches");
    expect(k).not.toContain("agentType");
  });

  it("contextPercent 按 0–1 比值换算成百分比", () => {
    expect(valueOf({ ...base, contextPercent: 0.72 }, "context")).toBe("72%");
  });

  it("半分钱以下的花费不成行", () => {
    expect(valueOf({ ...base, totalCostUsd: 0.001 }, "cost")).toBeUndefined();
    expect(valueOf({ ...base, totalCostUsd: 4.331 }, "cost")).toBe("$4.33");
  });

  it("pid 不精确时在值上标出来", () => {
    expect(valueOf({ ...base, pid: 42, pidPrecise: true }, "pid")).toBe("42");
    expect(valueOf({ ...base, pid: 42, pidPrecise: false }, "pid")).toContain("42");
    expect(valueOf({ ...base, pid: 42, pidPrecise: false }, "pid")).not.toBe("42");
  });

  it("接力棒次直接用 1 起的 hop", () => {
    const s = { ...base, handoff: { chainId: "c1", hop: 2, chainLen: 3 } };
    expect(valueOf(s, "handoff")).toBe("2/3");
  });

  it("子代理会话多出类型行", () => {
    const s = { ...base, isSubagent: true, agentType: "Explore" };
    expect(valueOf(s, "agentType")).toBe("Explore");
  });
});

describe("sourceLabel", () => {
  it("认识的源给展示名，缺席按 Claude", () => {
    expect(sourceLabel(undefined)).toBe("Claude");
    expect(sourceLabel("claude-code")).toBe("Claude");
    expect(sourceLabel("codex")).toBe("Codex");
    expect(sourceLabel("dsh")).toBe("dsh");
  });

  it("认不出的源原样显示，不静默兜底成 Claude", () => {
    expect(sourceLabel("kimi")).toBe("kimi");
  });
});

describe("resumeCommand", () => {
  it("只给 Claude 会话；codex / dsh 不给可能贴上去就报错的命令", () => {
    expect(resumeCommand(base)).toBe("claude --resume abc-123");
    expect(resumeCommand({ ...base, agentSource: "codex" })).toBeNull();
    expect(resumeCommand({ ...base, agentSource: "dsh" })).toBeNull();
  });
});
