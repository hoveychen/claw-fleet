import { describe, expect, it } from "vitest";
import type { DecisionHistoryRecord, FleetAskDecision, RawMessage, SessionInfo } from "../types";
import { inlineCodexFleetAsk, withCodexDecisionHistory } from "./codexDecision";

const session = (id: string, agentSource: string): SessionInfo =>
  ({ id, agentSource }) as SessionInfo;

const ask = (id: string, sessionId: string): FleetAskDecision =>
  ({
    kind: "fleet-ask",
    id,
    request: { id, sessionId, questions: [] },
  }) as FleetAskDecision;

const message = (uuid: string, timestamp: string): RawMessage => ({
  type: "assistant",
  uuid,
  timestamp,
  message: { role: "assistant", content: [{ type: "text", text: uuid }], stop_reason: "end_turn" },
});

const historicalAsk = (
  id: string,
  sessionId: string,
  requestedAt: string,
): DecisionHistoryRecord => ({
  kind: "fleet-ask",
  id,
  sessionId,
  workspaceName: "fleet",
  requestedAt,
  resolvedAt: "2026-09-06T12:00:02.000Z",
  outcome: "answered",
  questions: [{
    header: "选择",
    question: "采用哪种方案？",
    multiSelect: false,
    options: [{ label: "方案 A", description: "最小改动" }],
  }],
  answers: { "采用哪种方案？": "方案 A" },
});

describe("inlineCodexFleetAsk", () => {
  it("selects only the pending fleet ask belonging to the open Codex session", () => {
    const matching = ask("ask-2", "codex-session");
    expect(
      inlineCodexFleetAsk(session("codex-session", "codex"), [
        ask("ask-1", "another-session"),
        matching,
      ]),
    ).toBe(matching);
  });

  it("leaves direct-tool sources and unrelated sessions on their existing path", () => {
    const pending = ask("ask-1", "same-session");
    expect(inlineCodexFleetAsk(session("same-session", "claude-code"), [pending])).toBeNull();
    expect(inlineCodexFleetAsk(session("different-session", "codex"), [pending])).toBeNull();
    expect(inlineCodexFleetAsk(null, [pending])).toBeNull();
  });
});

describe("withCodexDecisionHistory", () => {
  it("inserts a resolved Codex fleet ask at its requested time", () => {
    const before = message("before", "2026-09-06T12:00:00.000Z");
    const after = message("after", "2026-09-06T12:00:03.000Z");

    const merged = withCodexDecisionHistory(
      session("codex-session", "codex"),
      [before, after],
      [historicalAsk("ask-1", "codex-session", "2026-09-06T12:00:01.000Z")],
    );

    expect(merged).toHaveLength(3);
    expect(merged[0]).toBe(before);
    expect(merged[2]).toBe(after);
    expect(merged[1]).toMatchObject({
      type: "assistant",
      uuid: "codex-decision-history-ask-1",
      timestamp: "2026-09-06T12:00:01.000Z",
      message: {
        role: "assistant",
        stop_reason: "end_turn",
        content: [{
          type: "tool_use",
          id: "ask-1",
          name: "mcp__fleet__fleet__ask",
          input: { questions: [{ question: "采用哪种方案？" }] },
        }],
      },
    });
  });

  it("ignores other sessions and decision ids already present in the transcript", () => {
    const existing: RawMessage = {
      type: "assistant",
      uuid: "existing",
      timestamp: "2026-09-06T12:00:01.000Z",
      message: {
        role: "assistant",
        content: [{ type: "tool_use", id: "ask-1", name: "mcp__fleet__fleet__ask", input: { questions: [] } }],
        stop_reason: "end_turn",
      },
    };

    expect(withCodexDecisionHistory(
      session("codex-session", "codex"),
      [existing],
      [
        historicalAsk("ask-1", "codex-session", "2026-09-06T12:00:01.000Z"),
        historicalAsk("ask-2", "other-session", "2026-09-06T12:00:02.000Z"),
      ],
    )).toEqual([existing]);
  });

  it("leaves non-Codex message arrays untouched", () => {
    const messages = [message("only", "2026-09-06T12:00:00.000Z")];
    expect(withCodexDecisionHistory(
      session("claude-session", "claude-code"),
      messages,
      [historicalAsk("ask-1", "claude-session", "2026-09-06T12:00:01.000Z")],
    )).toBe(messages);
  });
});
