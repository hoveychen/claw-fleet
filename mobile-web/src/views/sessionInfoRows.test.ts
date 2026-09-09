import { describe, expect, it } from "vitest";
import { buildInfoChips, resumeCommand } from "./sessionInfoRows";
import type { SessionInfo } from "../types";

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

describe("buildInfoChips", () => {
  it("给出工作区名", () => {
    expect(buildInfoChips(base)).toEqual(["proj"]);
  });

  it("路径与时间不进这行 chip：路径在半屏的复制行副行上，时间在每条消息旁", () => {
    const chips = buildInfoChips(base).join("|");
    expect(chips).not.toContain("/Users/x/workspace/proj");
    expect(chips).not.toContain(".jsonl");
    expect(chips).not.toContain("1700000000000");
  });

  it("会变的那些不搬进来——它们在状态轨和半屏的「此刻」/「进度」两节里", () => {
    const rich: SessionInfo = {
      ...base,
      pid: 42,
      entrypoint: "claw-fleet-newsession",
      slug: "fix/auth",
      runningSubagentCount: 3,
      handoff: { chainId: "c1", hop: 2, chainLen: 3 },
      watches: [{ id: "w1", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 1 }],
    };
    // 会话 id 也不在这行 chip 上：它太长，且它真正被用到的方式是复制走。
    expect(buildInfoChips(rich)).toEqual(["proj"]);
  });

  it("缺席的字段不产出 chip（不是产出一颗空的）", () => {
    expect(buildInfoChips(base)).not.toContain("claude-opus-5");
    expect(buildInfoChips({ ...base, model: "claude-opus-5" })[0]).toBe("claude-opus-5");
  });

  it("effort 紧跟在模型后面（桌面 header 有这颗 chip，手机不能没有）", () => {
    expect(buildInfoChips(base)).not.toContain("high");
    const chips = buildInfoChips({ ...base, model: "claude-opus-5", effort: "high" });
    expect(chips.indexOf("high")).toBe(chips.indexOf("claude-opus-5") + 1);
  });

  it("contextPercent 按 0–1 比值换算成百分比", () => {
    expect(buildInfoChips({ ...base, contextPercent: 0.72 }).join("|")).toContain("72%");
  });

  it("半分钱以下的花费不占一颗 chip", () => {
    expect(buildInfoChips({ ...base, totalCostUsd: 0.001 }).join("|")).not.toContain("$");
    expect(buildInfoChips({ ...base, totalCostUsd: 4.331 })).toContain("$4.33");
  });
});

describe("resumeCommand", () => {
  it("只给 Claude 会话；codex / dsh 不给可能贴上去就报错的命令", () => {
    expect(resumeCommand(base)).toBe("claude --resume abc-123");
    expect(resumeCommand({ ...base, agentSource: "codex" })).toBeNull();
    expect(resumeCommand({ ...base, agentSource: "dsh" })).toBeNull();
  });
});
