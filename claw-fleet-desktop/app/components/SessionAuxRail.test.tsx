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
  document.body.querySelectorAll("[class*='menu']").forEach((n) => n.remove());
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
        workspacePath="/repo"
        onOpenAgent={() => {}}
        onToggleAgent={() => {}}
        onCloseAgent={() => {}}
        onToggleDoc={() => {}}
        onCloseDoc={() => {}}
        onCloseOtherDocs={() => {}}
        onCloseAllDocs={() => {}}
        onCollapseDoc={() => {}}
        onHideRail={() => {}}
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
    expect(card.querySelector("[class*='aux_doc_pane']")).not.toBeNull();
    expect(el.querySelectorAll("aside > *")).toHaveLength(1);
  });

  // An expanded doc card's header is the reader's own AuxDocBar. It used to be
  // a strip *above* that bar, which printed the doc's name twice in a row. (An
  // agent card keeps its strip — a transcript has no header to borrow.)
  it("prints the expanded doc's name once, not in a strip of its own", () => {
    const doc = makeAuxDoc("wiki", "arch/overview");
    const el = render({ docs: [doc], expandedId: doc.id });

    expect(el.querySelector("[class*='doc_card_head']")).toBeNull();
  });

  // A chip's disambiguator: `auxDocMeta` reads it off the ref, so two same-named
  // files from different directories are still told apart at a glance.
  it("gives a collapsed chip the value that tells it apart from its namesakes", () => {
    const el = render({ docs: [makeAuxDoc("file", "/repo/src/gui/mod.rs")] });

    expect(el.querySelector("[class*='doc_card_meta']")?.textContent).toBe("gui");
  });

  // With no handler, a right-click anywhere in the rail bubbled to the app-wide
  // menu (contextMenu.ts) and answered a request to act on a document with
  // Settings / About / Quit.
  it("answers a right-click on a chip with the card's own menu", () => {
    const doc = makeAuxDoc("file", "/repo/src/main.rs");
    const el = render({ docs: [doc] });
    const chip = el.querySelector("[class*='doc_card']") as HTMLElement;
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });

    act(() => void chip.dispatchEvent(ev));

    expect(ev.defaultPrevented).toBe(true);
    // Portalled to the body, so it is not under `container`.
    const menu = document.body.querySelector("[class*='menu']") as HTMLElement;
    expect(menu.textContent).toContain("/repo/src/main.rs");
  });

  // Regression guard for a bug the unit tests could not see and the browser
  // pass caught: `.rail` is `pointer-events: none` (so the transcript keeps the
  // clicks between cards), which means an `onContextMenu` on the <aside> is
  // dead in the app while passing in jsdom, where there is no hit-testing. The
  // stack-level actions therefore have to live on the cards.
  it("puts hiding the rail on a card's menu, not on the rail's dead background", () => {
    const el = render({ docs: [makeAuxDoc("file", "/repo/a.rs")] });
    const chip = el.querySelector("[class*='doc_card']") as HTMLElement;

    act(() =>
      void chip.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true })),
    );
    const labels = Array.from(document.body.querySelectorAll("[class*='menu'] button")).map(
      (b) => b.textContent ?? "",
    );

    expect(labels.some((l) => /辅助栏|side rail/i.test(l))).toBe(true);
    // And the aside itself must not carry one, or the fix rots back.
    const rail = el.querySelector("aside") as HTMLElement;
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    act(() => void rail.dispatchEvent(ev));
    expect(ev.defaultPrevented).toBe(false);
  });

  // The one state with no card to right-click: held open by the switch with
  // nothing in play. That line IS a `.rail > *`, so it does get the events.
  it("carries the stack menu on the empty-rail line", () => {
    const onHideRail = vi.fn();
    const el = render({ open: true, onHideRail });
    const line = el.querySelector("aside > p") as HTMLElement;
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });

    act(() => void line.dispatchEvent(ev));
    expect(ev.defaultPrevented).toBe(true);
    const items = Array.from(document.body.querySelectorAll("[class*='menu'] button"));
    expect(items).toHaveLength(1);
    act(() => (items[0] as HTMLElement).click());
    expect(onHideRail).toHaveBeenCalled();
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
