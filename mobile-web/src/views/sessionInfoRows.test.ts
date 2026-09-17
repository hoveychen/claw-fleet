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
  it("shows workspace name", () => {
    expect(buildInfoChips(base)).toEqual(["proj"]);
  });

  it("excludes path and time: path is in the copy row beside half-screen, time is beside each message", () => {
    const chips = buildInfoChips(base).join("|");
    expect(chips).not.toContain("/Users/x/workspace/proj");
    expect(chips).not.toContain(".jsonl");
    expect(chips).not.toContain("1700000000000");
  });

  it("excludes dynamic fields—they live in the status bar and the 'now'/'progress' sections", () => {
    const rich: SessionInfo = {
      ...base,
      pid: 42,
      entrypoint: "claw-fleet-newsession",
      slug: "fix/auth",
      runningSubagentCount: 3,
      handoff: { chainId: "c1", hop: 2, chainLen: 3 },
      watches: [{ id: "w1", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 1, structuralFailStreak: 0 }],
    };
    // Session id also skips this chip row: it's too long and its actual use is copying it out.
    expect(buildInfoChips(rich)).toEqual(["proj"]);
  });

  it("missing fields produce no chip (not an empty one)", () => {
    expect(buildInfoChips(base)).not.toContain("claude-opus-5");
    expect(buildInfoChips({ ...base, model: "claude-opus-5" })[0]).toBe("claude-opus-5");
  });

  it("effort immediately follows model (desktop header has this chip, phone must too)", () => {
    expect(buildInfoChips(base)).not.toContain("high");
    const chips = buildInfoChips({ ...base, model: "claude-opus-5", effort: "high" });
    expect(chips.indexOf("high")).toBe(chips.indexOf("claude-opus-5") + 1);
  });

  it("converts contextPercent from 0–1 ratio to percentage", () => {
    expect(buildInfoChips({ ...base, contextPercent: 0.72 }).join("|")).toContain("72%");
  });

  it("suppresses chips for costs below half a cent", () => {
    expect(buildInfoChips({ ...base, totalCostUsd: 0.001 }).join("|")).not.toContain("$");
    expect(buildInfoChips({ ...base, totalCostUsd: 4.331 })).toContain("$4.33");
  });
});

describe("resumeCommand", () => {
  it("only for Claude sessions; codex/dsh skip commands that might error if pasted", () => {
    expect(resumeCommand(base)).toBe("claude --resume abc-123");
    expect(resumeCommand({ ...base, agentSource: "codex" })).toBeNull();
    expect(resumeCommand({ ...base, agentSource: "dsh" })).toBeNull();
  });
});
