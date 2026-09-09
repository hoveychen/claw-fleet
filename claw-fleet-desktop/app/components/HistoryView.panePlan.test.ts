// What the detail column shows, as a pure decision.
//
// The bug this pins: pressing 「1 张卡等你回复」 on fleet-cloud landed on
// 「新建会话」. The pill sets `openId` to the card's session id unconditionally;
// when that id is nowhere in the scan the pane used to fall past every branch
// into its resting state, which *is* the new-session composer. Nothing on
// screen said why, and in simplified mode — which mounts no `DecisionPanel` —
// the card had no surface anywhere at all.
//
// A session can be missing from the scan for three unrelated reasons: the
// transcript was deleted, it aged past the scanner's mtime cap, or it is
// unreadable by the user the server runs as (the fleet-cloud case: a root-
// spawned session wrote a 0600 transcript that `fleet webui`, running as
// `fleet`, cannot open). All three land here, so the fix is at this branch and
// not at any one cause.
import { describe, expect, it } from "vitest";

import { panePlan } from "./HistoryView";
import type { PendingDecision, SessionInfo } from "../types";

const DRAFT_ID = "new:draft";

function session(id: string): SessionInfo {
  return { id, jsonlPath: `/p/${id}.jsonl` } as unknown as SessionInfo;
}

function card(id: string, sessionId: string): PendingDecision {
  return {
    kind: "fleet-ask",
    id,
    request: { id, sessionId, questions: [] },
    answers: {},
    arrivedAt: 1,
  } as unknown as PendingDecision;
}

function plan(over: Partial<Parameters<typeof panePlan>[0]> = {}) {
  return panePlan({
    openId: null,
    activeSession: null,
    scanReady: true,
    simplifiedMode: true,
    decisions: [],
    ...over,
  });
}

describe("panePlan", () => {
  it("shows the session when the scan has it", () => {
    expect(plan({ openId: "s1", activeSession: session("s1") })).toEqual({ kind: "session" });
  });

  it("shows the composer with nothing open", () => {
    expect(plan()).toEqual({ kind: "resting" });
  });

  it("shows the draft for the composer sentinel", () => {
    expect(plan({ openId: DRAFT_ID })).toEqual({ kind: "draft" });
  });

  // The regression. Before the fix this returned the resting state, i.e. the
  // new-session composer, for exactly the id the pending-cards pill hands over.
  it("does NOT fall back to the composer for a session the scan cannot see", () => {
    const p = plan({ openId: "gone", decisions: [card("c1", "gone")] });
    expect(p.kind).not.toBe("resting");
    expect(p.kind).not.toBe("draft");
  });

  it("renders the card in the pane in simplified mode — its only surface there", () => {
    const c = card("c1", "gone");
    expect(plan({ openId: "gone", decisions: [c] })).toEqual({
      kind: "orphan-card",
      sessionId: "gone",
      decisions: [c],
    });
  });

  it("carries only that session's cards, not every pending one", () => {
    const mine = card("c1", "gone");
    const theirs = card("c2", "other");
    const p = plan({ openId: "gone", decisions: [theirs, mine] });
    expect(p).toEqual({ kind: "orphan-card", sessionId: "gone", decisions: [mine] });
  });

  // Full mode mounts `DecisionPanel`, which already draws this card as an
  // overlay. Drawing it here too would be a duplicate card — its own bug.
  it("only explains the empty pane in full mode, leaving the card to the overlay", () => {
    expect(
      plan({ openId: "gone", simplifiedMode: false, decisions: [card("c1", "gone")] }),
    ).toEqual({ kind: "orphan-note", sessionId: "gone", hasCard: true });
  });

  it("says the session is missing when no card belongs to it", () => {
    expect(plan({ openId: "gone", decisions: [card("c1", "other")] })).toEqual({
      kind: "orphan-note",
      sessionId: "gone",
      hasCard: false,
    });
  });

  // Before the first scan lands every id looks orphaned. Accusing a session of
  // being missing then would flash the notice on every cold start.
  it("holds the resting state until the first scan lands", () => {
    expect(
      plan({ openId: "s1", scanReady: false, decisions: [card("c1", "s1")] }),
    ).toEqual({ kind: "resting" });
  });
});
