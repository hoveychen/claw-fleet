import { describe, expect, it } from "vitest";
import {
  canResumeSession,
  HANDOFF_ENTRYPOINT,
  NEW_SESSION_ENTRYPOINT,
  type SessionInfo,
  type SessionStatus,
} from "./types";

function session(over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id: "s1",
    workspacePath: "/w",
    workspaceName: "w",
    ideName: null,
    entrypoint: NEW_SESSION_ENTRYPOINT,
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: null,
    aiTitle: null,
    status: "idle",
    tokenSpeed: 0,
    agentTokenSpeed: 0,
    totalOutputTokens: 0,
    totalCostUsd: 0,
    agentTotalCostUsd: 0,
    costSpeedUsdPerMin: 0,
    lastMessagePreview: null,
    lastActivityMs: 0,
    createdAtMs: 0,
    jsonlPath: "/w/s1.jsonl",
    model: null,
    thinkingLevel: null,
    pid: null,
    pidPrecise: false,
    procAlive: false,
    lastSkill: null,
    contextPercent: null,
    agentSource: "claude-code",
    lastOutcome: null,
    ...over,
  } as SessionInfo;
}

describe("canResumeSession", () => {
  it("offers the composer right after a headless turn ends (waitingInput, process gone)", () => {
    // `claude -p` exits at end_turn, but the status stays `waitingInput` for
    // 5 minutes — exactly the window in which the human wants to follow up.
    expect(
      canResumeSession(session({ status: "waitingInput", procAlive: false })),
    ).toBe(true);
  });

  it("withholds the composer while the process is alive but parked (waitingInput, procAlive)", () => {
    expect(
      canResumeSession(session({ status: "waitingInput", procAlive: true })),
    ).toBe(false);
  });

  it("withholds the composer for every genuinely in-flight status", () => {
    const inFlight: SessionStatus[] = [
      "thinking",
      "executing",
      "streaming",
      "processing",
      "active",
      "delegating",
    ];
    for (const status of inFlight) {
      expect(canResumeSession(session({ status, procAlive: true }))).toBe(false);
    }
  });

  it("offers the composer for an idle Fleet session and for handoff successors", () => {
    expect(canResumeSession(session({ status: "idle" }))).toBe(true);
    expect(
      canResumeSession(session({ entrypoint: HANDOFF_ENTRYPOINT })),
    ).toBe(true);
  });

  it("never offers the composer for subagents or foreign entrypoints", () => {
    expect(canResumeSession(session({ isSubagent: true }))).toBe(false);
    expect(canResumeSession(session({ entrypoint: "cli" }))).toBe(false);
  });
});
