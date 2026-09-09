// @vitest-environment jsdom
//
// Strings are the English ones: `../i18n` initialises to `en` under vitest.
//
// Simplified mode does not mount the `DecisionPanel`, so this pill is the only
// always-visible sign that a card is waiting on a task you do not have open.
// Boss hit the gap on 2026-09-09: a chime every 10s with no card anywhere on
// screen. What can go wrong here is the header not subscribing to the decision
// store, or the click not naming a session — either one puts him back to
// hunting for the 「待决策」row chip.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import "../i18n";
import { SimpleNavigation } from "./SimpleNavigation";
import { useDecisionStore, useUIStore } from "../store";
import type { PendingDecision } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

function card(id: string, sessionId: string, arrivedAt: number, parked = false): PendingDecision {
  return {
    kind: "fleet-ask",
    id,
    request: {
      id,
      sessionId,
      workspaceName: "ws",
      aiTitle: `task ${sessionId}`,
      timestamp: new Date().toISOString(),
      parked,
      questions: [{ question: "?", header: "h", multiSelect: false, options: [] }],
    },
    answers: {},
    arrivedAt,
  } as unknown as PendingDecision;
}

function render() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<SimpleNavigation />));
}

function pill(): HTMLButtonElement | null {
  return container!.querySelector("header > button:not([aria-label])") as HTMLButtonElement | null;
}

beforeEach(() => {
  useDecisionStore.setState({ decisions: [], activeDecisionId: null });
  useUIStore.setState({ openTaskNav: null });
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
  root = null;
});

describe("simplified-mode header decisions pill", () => {
  it("is absent while nothing is pending", () => {
    render();
    expect(pill()).toBeNull();
  });

  it("appears and counts cards that arrive after mount", () => {
    render();
    act(() => {
      useDecisionStore.setState({ decisions: [card("c1", "s1", 10), card("c2", "s2", 20)] });
    });
    expect(pill()?.textContent).toBe("Waiting on you: 2");
  });

  it("opens the oldest card's task when clicked", () => {
    render();
    // s-old arrived first but is listed last, so a click that just took
    // decisions[0] would land on the wrong task.
    act(() => {
      useDecisionStore.setState({
        decisions: [card("c2", "s-new", 200), card("c1", "s-old", 100)],
      });
    });
    act(() => pill()!.click());
    expect(useUIStore.getState().openTaskNav?.sessionId).toBe("s-old");
    // The click also has to leave simplified mode on a page that renders the
    // task, which `requestOpenTask` does by hopping to 任务 (history).
    expect(useUIStore.getState().viewMode).toBe("history");
  });

  it("reads as timed out when any card is parked", () => {
    render();
    act(() => {
      useDecisionStore.setState({ decisions: [card("c1", "s1", 10, true)] });
    });
    expect(pill()?.textContent).toBe("Timed out: 1");
  });
});
