import { describe, expect, it } from "vitest";
import {
  canResumeSession,
  isQuietAlive,
  rowBarColor,
  shouldFollowSession,
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
    agentLastActivityMs: 0,
    runningSubagentCount: 0,
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

  it("offers the composer for a Fleet-launched codex session, not a foreign codex one", () => {
    // A Fleet-spawned codex session carries originator "fleet" on `entrypoint`
    // and resumes via `codex exec resume` (M2/M3).
    expect(
      canResumeSession(
        session({ agentSource: "codex", entrypoint: "fleet", status: "idle" }),
      ),
    ).toBe(true);
    // Bare `codex exec` / the VS Code extension are read-only here.
    expect(
      canResumeSession(session({ agentSource: "codex", entrypoint: "codex_exec" })),
    ).toBe(false);
  });
});

describe("isQuietAlive / rowBarColor third state", () => {
  it("marks a live process whose transcript went quiet as quiet-alive", () => {
    // The scan-computed status ages out on a hard clock (`determine_status`:
    // tool_use → Executing for 60s, then Idle), so a session sitting on one
    // long Bash — a build, a background-task wait — reads `idle` while its
    // process is very much alive. That is the case the row dot must not paint
    // as "ended".
    expect(isQuietAlive(session({ status: "idle", procAlive: true }))).toBe(true);
  });

  it("is not quiet-alive when the status still says something is going on", () => {
    expect(isQuietAlive(session({ status: "executing", procAlive: true }))).toBe(false);
    expect(isQuietAlive(session({ status: "waitingInput", procAlive: true }))).toBe(false);
  });

  it("is not quiet-alive once the process is gone", () => {
    expect(isQuietAlive(session({ status: "idle", procAlive: false }))).toBe(false);
  });

  it("paints quiet-alive rows a faded green, distinct from both live and ended", () => {
    expect(rowBarColor(session({ status: "idle", procAlive: true }))).toBe(
      "rgba(var(--color-success-rgb), 0.45)",
    );
    expect(rowBarColor(session({ status: "executing", procAlive: true }))).toBe(
      "var(--color-success)",
    );
    expect(rowBarColor(session({ status: "idle", procAlive: false }))).toBe(null);
  });
});

describe("shouldFollowSession", () => {
  it("follows in-flight and waiting statuses", () => {
    expect(shouldFollowSession(session({ status: "thinking" }))).toBe(true);
    expect(shouldFollowSession(session({ status: "executing" }))).toBe(true);
    expect(shouldFollowSession(session({ status: "waitingInput" }))).toBe(true);
  });

  it("does not follow a genuinely finished session", () => {
    expect(shouldFollowSession(session({ status: "idle", procAlive: false }))).toBe(
      false,
    );
  });

  it("keeps following while the process is alive, whatever the status says", () => {
    // Regression (2026-07-16 "codex 假死"): a long codex turn misread as Idle
    // disarmed the detail poller — "自动跟随中" stopped pulling new messages
    // while the codex process was still working. A live process can produce new
    // transcript writes regardless of the scan-computed status, so status alone
    // must never stop the follow poller.
    expect(shouldFollowSession(session({ status: "idle", procAlive: true }))).toBe(
      true,
    );
  });
});
