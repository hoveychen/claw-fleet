import { describe, expect, it } from "vitest";
import type { FleetAskDecision, SessionInfo } from "../types";
import { inlineCodexFleetAsk } from "./codexDecision";

const session = (id: string, agentSource: string): SessionInfo =>
  ({ id, agentSource }) as SessionInfo;

const ask = (id: string, sessionId: string): FleetAskDecision =>
  ({
    kind: "fleet-ask",
    id,
    request: { id, sessionId, questions: [] },
  }) as FleetAskDecision;

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
