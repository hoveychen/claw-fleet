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

const keys = (s: SessionInfo) => buildInfoRows(s, 1_700_000_060_000).map((r) => r.key);
const valueOf = (s: SessionInfo, key: string) =>
  buildInfoRows(s, 1_700_000_060_000).find((r) => r.key === key)?.value;

describe("buildInfoRows", () => {
  it("总是给出身份三件套：会话 ID、工作区路径、会话记录", () => {
    expect(valueOf(base, "sessionId")).toBe("abc-123");
    expect(valueOf(base, "workspacePath")).toBe("/Users/x/workspace/proj");
    expect(valueOf(base, "jsonlPath")).toBe(base.jsonlPath);
  });

  it("只放身份字段 —— 状态/用量/接力这些在别处已有更好的呈现", () => {
    const rich: SessionInfo = {
      ...base,
      contextPercent: 0.72,
      totalCostUsd: 4.33,
      pid: 42,
      entrypoint: "claw-fleet-newsession",
      slug: "fix/auth",
      agentSource: "codex",
      runningSubagentCount: 3,
      handoff: { chainId: "c1", hop: 2, chainLen: 3 },
      watches: [
        { id: "w1", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 1 },
      ],
    };
    expect(keys(rich).sort()).toEqual(
      ["created", "jsonlPath", "lastActivity", "sessionId", "workspacePath"].sort(),
    );
  });

  it("缺席的字段不占行（不显示成空值）", () => {
    expect(keys(base)).not.toContain("model");
    expect(valueOf({ ...base, model: "claude-opus-5" }, "model")).toBe("claude-opus-5");
  });

  it("最后活动同时给相对与绝对时刻", () => {
    const v = valueOf(base, "lastActivity") ?? "";
    expect(v).toContain("·");
    expect(v.length).toBeGreaterThan(6);
  });
});

describe("resumeCommand", () => {
  it("只给 Claude 会话；codex / dsh 不给可能贴上去就报错的命令", () => {
    expect(resumeCommand(base)).toBe("claude --resume abc-123");
    expect(resumeCommand({ ...base, agentSource: "codex" })).toBeNull();
    expect(resumeCommand({ ...base, agentSource: "dsh" })).toBeNull();
  });
});
