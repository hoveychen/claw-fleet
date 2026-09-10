// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import i18n from "../i18n";
import { agentCardId, makeAuxDoc } from "../detailAux";
import type { SessionInfo } from "../types";
import { SessionAuxRail } from "./SessionAuxRail";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function agent(id: string, title: string): SessionInfo {
  return {
    id,
    aiTitle: title,
    status: "executing",
    isSubagent: true,
    lastActivityMs: Date.now(),
    agentTokenSpeed: 0,
  } as unknown as SessionInfo;
}

function render(props: Partial<Parameters<typeof SessionAuxRail>[0]> = {}) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <SessionAuxRail
        open
        agents={[]}
        docs={[]}
        expandedId={null}
        onOpenAgent={() => {}}
        onToggleAgent={() => {}}
        onCloseAgent={() => {}}
        onToggleDoc={() => {}}
        onCloseDoc={() => {}}
        onOpenWiki={() => {}}
        cardWidth={420}
        onGripDown={() => {}}
        {...props}
      />,
    ),
  );
  return container;
}

describe("SessionAuxRail", () => {
  // The reason the rail can be permanent: closed, it is not a narrow empty
  // frame, it is not there at all. SessionDetail closes it by default whenever
  // nothing is in play.
  it("costs no width when closed", () => {
    expect(render({ open: false }).childElementCount).toBe(0);
  });

  // Held open by the header switch on a session with nothing in play: say so,
  // rather than showing a blank column that reads as a failure to load.
  it("says why it is empty when the reader pinned it open", () => {
    const el = render({ open: true });

    expect(el.querySelector("aside")).not.toBeNull();
    expect(el.querySelector("aside > p")?.textContent).toBeTruthy();
  });

  it("shows a card per live subagent", () => {
    const el = render({ agents: [agent("a", "Trace the watcher")] });

    expect(el.querySelector("aside")).not.toBeNull();
    expect(el.textContent).toContain("Trace the watcher");
  });

  // Cards only, one column, no tab strip and no section headings — the two
  // kinds are siblings in the same stack.
  it("stacks doc cards with the agent cards and offers no tabs", () => {
    const el = render({
      agents: [agent("a", "Trace the watcher")],
      docs: [makeAuxDoc("file", "/repo/src/main.rs")],
    });

    expect(el.querySelector('[role="tablist"]')).toBeNull();
    expect(el.textContent).toContain("main.rs");
    expect(el.querySelectorAll("aside > *")).toHaveLength(2);
  });

  it("expands a doc card in place, and dismisses it from its own ✕", () => {
    const onToggleDoc = vi.fn();
    const onCloseDoc = vi.fn();
    const doc = makeAuxDoc("wiki", "arch/overview");
    const el = render({ docs: [doc], onToggleDoc, onCloseDoc });
    const [open, close] = Array.from(el.querySelectorAll("button")) as HTMLElement[];

    act(() => open.click());
    act(() => close.click());
    expect(onToggleDoc).toHaveBeenCalledWith(doc.id);
    expect(onCloseDoc).toHaveBeenCalledWith(doc.id);
  });

  // The whole point of the change: a doc is read in its own card, and the
  // drawer — which used to carry it, under a duplicate copy of this same
  // name — is not involved. The reader lands inside the card's own element.
  it("reads the expanded doc inside the card, with a width grip", () => {
    const doc = makeAuxDoc("wiki", "arch/overview");
    const el = render({ docs: [doc], expandedId: doc.id });
    const card = el.querySelector("aside > div:last-child") as HTMLElement;

    expect(card.querySelector('[role="separator"]')).not.toBeNull();
    // WikiTabPane mounts inside the card rather than in a drawer beside it.
    expect(card.childElementCount).toBeGreaterThan(2);
    expect(el.querySelectorAll("aside > *")).toHaveLength(1);
  });

  // The behaviour this rail change is for: a subagent is read here, not by
  // leaving for its own session view.
  it("clicking a subagent card expands it instead of navigating", () => {
    const onToggleAgent = vi.fn();
    const onOpenAgent = vi.fn();
    const a = agent("sub-1", "Trace the watcher");
    const el = render({ agents: [a], onToggleAgent, onOpenAgent });

    act(() => (el.querySelector("button") as HTMLElement).click());
    expect(onToggleAgent).toHaveBeenCalledWith(a);
    expect(onOpenAgent).not.toHaveBeenCalled();
  });

  it("gives the expanded subagent card a width grip and a way out to its page", () => {
    const onOpenAgent = vi.fn();
    const a = agent("sub-1", "Trace the watcher");
    const el = render({ agents: [a], expandedId: agentCardId("sub-1"), onOpenAgent });
    const card = el.querySelector("aside > div") as HTMLElement;

    expect(card.querySelector('[role="separator"]')).not.toBeNull();
    // The transcript pane mounts inside the card: grip + head + pane.
    expect(card.childElementCount).toBeGreaterThan(2);

    const goto = card.querySelector(
      `[aria-label="${i18n.t("detail.agent_card_goto")}"]`,
    ) as HTMLElement;
    act(() => goto.click());
    expect(onOpenAgent).toHaveBeenCalledWith(a);
  });

  // A live agent's chip is derived from the scan, so dismissing it would last
  // until the next tick. Only the expanded card (and a pinned leftover) offers
  // the ✕.
  it("offers no ✕ on a live agent's collapsed chip", () => {
    const el = render({ agents: [agent("sub-1", "Trace the watcher")] });
    expect(el.textContent).not.toContain("✕");
  });
});
