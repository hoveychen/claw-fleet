import { describe, expect, it } from "vitest";
import {
  CODEX_FLEET_ORIGINATOR,
  canResumeSession,
  isFleetOwnedEntrypoint,
  type SessionInfo,
} from "./types";

/** Minimal ended-session fixture; overrides fill in the fields under test. */
function mk(overrides: Partial<SessionInfo>): SessionInfo {
  return {
    id: "s1",
    workspacePath: "/w",
    workspaceName: "w",
    status: "waitingInput",
    isSubagent: false,
    lastActivityMs: 0,
    createdAtMs: 0,
    jsonlPath: "/w/s1.jsonl",
    procAlive: false,
    ...overrides,
  } as SessionInfo;
}

describe("isFleetOwnedEntrypoint", () => {
  it("认得 Fleet 起的 Claude 会话（新会话 / handoff）", () => {
    expect(isFleetOwnedEntrypoint("claw-fleet-newsession")).toBe(true);
    expect(isFleetOwnedEntrypoint("claw-fleet-handoff")).toBe(true);
  });

  it("认得 Fleet 起的 Codex 会话（originator === \"fleet\"）", () => {
    expect(isFleetOwnedEntrypoint(CODEX_FLEET_ORIGINATOR)).toBe(true);
    expect(isFleetOwnedEntrypoint("fleet")).toBe(true);
  });

  it("不把外部来源当成 Fleet 自己起的", () => {
    expect(isFleetOwnedEntrypoint("cli")).toBe(false);
    expect(isFleetOwnedEntrypoint("codex_exec")).toBe(false);
    expect(isFleetOwnedEntrypoint(null)).toBe(false);
    expect(isFleetOwnedEntrypoint(undefined)).toBe(false);
  });
});

describe("canResumeSession", () => {
  it("Fleet 起的 codex 会话（进程已结束、非 in-flight）可续接", () => {
    const codex = mk({ entrypoint: "fleet", agentSource: "codex" });
    expect(canResumeSession(codex)).toBe(true);
  });

  it("外部 codex_exec 会话不可续接", () => {
    const external = mk({ entrypoint: "codex_exec", agentSource: "codex" });
    expect(canResumeSession(external)).toBe(false);
  });
});
