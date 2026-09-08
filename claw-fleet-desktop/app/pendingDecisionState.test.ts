import { describe, it, expect } from "vitest";
import { pendingDecisionState } from "./pendingDecisionState";
import type { PendingDecision } from "./types";

function card(sessionId: string, parked = false): PendingDecision {
  return {
    kind: "fleet-ask",
    id: `${sessionId}-${parked ? "parked" : "live"}`,
    request: { sessionId, parked },
    answers: {},
    arrivedAt: 0,
  } as unknown as PendingDecision;
}

describe("pendingDecisionState", () => {
  it("is none for a session with no card", () => {
    expect(pendingDecisionState([card("other")], "mine")).toBe("none");
  });

  it("is pending while the card is still waiting", () => {
    expect(pendingDecisionState([card("mine")], "mine")).toBe("pending");
  });

  it("is parked once the wait timed out", () => {
    expect(pendingDecisionState([card("mine", true)], "mine")).toBe("parked");
  });

  it("reports parked even when a live card is listed first", () => {
    expect(pendingDecisionState([card("mine"), card("mine", true)], "mine")).toBe("parked");
  });
});
