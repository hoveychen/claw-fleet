// @vitest-environment jsdom
//
// Strings are the English ones: `../i18n` initialises to `en` under vitest.
//
// The chip's *wiring*. `pendingDecisionState.test.ts` covers the rule; what can
// still go wrong here is the row not subscribing to the decision store at all
// (SessionRow is memoised on props, so a card arriving for a session whose row
// props did not change must still repaint it).
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import "../i18n";
import { SessionRow } from "./SessionRow";
import { useDecisionStore } from "../store";
import { MOCK_SESSIONS } from "../mock/data";
import type { PendingDecision } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

const session = MOCK_SESSIONS[0];

function card(sessionId: string, parked: boolean): PendingDecision {
  return {
    kind: "fleet-ask",
    id: `card-${parked ? "parked" : "live"}`,
    request: {
      id: `card-${parked ? "parked" : "live"}`,
      sessionId,
      workspaceName: "ws",
      timestamp: new Date().toISOString(),
      parked,
      questions: [{ question: "?", header: "h", multiSelect: false, options: [] }],
    },
    answers: {},
    arrivedAt: 0,
  } as unknown as PendingDecision;
}

function render() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <SessionRow
        session={session}
        snippet={undefined}
        isSelected={false}
        isOpen={false}
        nowTick={0}
        showSource={false}
        onClick={() => {}}
        onContextMenu={() => {}}
      />,
    ),
  );
  return container;
}

beforeEach(() => {
  useDecisionStore.setState({ decisions: [] });
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
  root = null;
  useDecisionStore.setState({ decisions: [] });
});

describe("SessionRow decision chip", () => {
  it("shows nothing while the task has no card", () => {
    expect(render().textContent).not.toContain("Decision");
  });

  it("shows the pending chip for a live card, and flips to timed-out when it parks", () => {
    const el = render();
    act(() => {
      useDecisionStore.setState({ decisions: [card(session.id, false)] });
    });
    expect(el.textContent).toContain("Decision");
    expect(el.textContent).not.toContain("Timed out");

    // The park flip is an in-place mutation of the same card — the row must
    // repaint even though its props are untouched.
    act(() => {
      useDecisionStore.setState({ decisions: [card(session.id, true)] });
    });
    expect(el.textContent).toContain("Timed out");
  });

  it("ignores a card belonging to another session", () => {
    const el = render();
    act(() => {
      useDecisionStore.setState({ decisions: [card("someone-else", true)] });
    });
    expect(el.textContent).not.toContain("Timed out");
  });
});
