import { describe, expect, it } from "vitest";
import { agentLabel, agentIdTail } from "./agentScope";
import type { SessionInfo } from "../types";

/** Minimal row: only the fields `agentLabel` reads. */
function row(over: Partial<SessionInfo>): SessionInfo {
  return { id: "s1", isSubagent: false, ...over } as SessionInfo;
}

describe("agentLabel", () => {
  it("names the main process", () => {
    expect(agentLabel(row({ isSubagent: false }))).toMatch(/◈/);
  });

  it("prefers the agent type when one was recorded", () => {
    expect(agentLabel(row({ isSubagent: true, agentType: "Explore" }))).toContain("Explore");
  });

  // The bug this covers: codex `thread_spawn` records `agent_role: null`, so
  // three concurrent codex subagents all rendered as the bare word 子代理 (Subagent)
  // and only the id tail told them apart. Core already carries their nickname on aiTitle.
  it("falls back to the codex nickname on aiTitle", () => {
    const s = row({ isSubagent: true, agentSource: "codex", aiTitle: "Kuhn" });
    expect(agentLabel(s)).toContain("Kuhn");
  });

  it("keeps the generic word for a claude subagent with no type", () => {
    const s = row({ isSubagent: true, agentSource: "claude", aiTitle: "某个很长的会话标题" });
    expect(agentLabel(s)).toMatch(/⎇ (子代理|Subagent)/);
    expect(agentLabel(s)).not.toContain("很长");
  });
});

describe("agentIdTail", () => {
  it("takes the tail so same-prefix ids diverge", () => {
    expect(agentIdTail("agent-0123456789abcdef")).toBe("#abcdef");
  });
});
