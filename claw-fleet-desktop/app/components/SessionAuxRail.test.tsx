// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import i18n from "../i18n";
import { agentCardId, makeAuxDoc } from "../detailAux";
import type { SessionInfo } from "../types";
import { ReaderHostContext } from "./readerHost";
import { SessionAuxRail } from "./SessionAuxRail";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let readerHost: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  readerHost?.remove();
  readerHost = null;
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

/** Renders the sections into `container`, and any expanded reader into a
 *  separate host element the way SessionDetail does. */
function render(props: Partial<Parameters<typeof SessionAuxRail>[0]> = {}) {
  container = document.createElement("div");
  readerHost = document.createElement("div");
  document.body.appendChild(container);
  document.body.appendChild(readerHost);
  root = createRoot(container);
  act(() =>
    root!.render(
      <ReaderHostContext.Provider value={readerHost}>
      <SessionAuxRail
        agents={[]}
        docs={[]}
        explains={[]}
        expandedId={null}
        workspacePath="/repo"
        onOpenAgent={() => {}}
        onToggleAgent={() => {}}
        onCloseAgent={() => {}}
        onToggleDoc={() => {}}
        onCloseDoc={() => {}}
        onCloseOtherDocs={() => {}}
        onCloseAllDocs={() => {}}
        onHideRail={() => {}}
        onOpenWiki={() => {}}
        onToggleExplain={() => {}}
        onCloseExplain={() => {}}
        onLocateExplain={() => {}}
        onFollowUpExplain={() => {}}
        onGripDown={() => {}}
        {...props}
      />
      </ReaderHostContext.Provider>,
    ),
  );
  return container;
}

describe("SessionAuxRail", () => {
  // Each section is omitted while empty; with nothing in play there is
  // nothing to render, and TaskRail says why if the switch holds it open.
  it("renders nothing when nothing is in play", () => {
    expect(render().childElementCount).toBe(0);
  });

  it("shows a row per live subagent under its own heading", () => {
    const el = render({ agents: [agent("a", "Trace the watcher")] });

    expect(el.querySelectorAll("section")).toHaveLength(1);
    expect(el.querySelector("h3")?.textContent).toContain(i18n.t("home.rail_agents"));
    expect(el.textContent).toContain("Trace the watcher");
  });

  // One section per kind, as list rows in the rail — no tab strip.
  it("gives docs a section of their own below the agents, and offers no tabs", () => {
    const el = render({
      agents: [agent("a", "Trace the watcher")],
      docs: [makeAuxDoc("file", "/repo/src/main.rs")],
    });

    expect(el.querySelector('[role="tablist"]')).toBeNull();
    const sections = el.querySelectorAll("section");
    expect(sections).toHaveLength(2);
    expect(sections[0].textContent).toContain("Trace the watcher");
    expect(sections[1].textContent).toContain("main.rs");
  });

  // The rail is a near-permanent column, so a collapsed row has no ✕: closing
  // is something you do to an open reader, not a way to hide the row.
  it("expands a doc card in place, and offers no ✕ on the collapsed row", () => {
    const onToggleDoc = vi.fn();
    const onCloseDoc = vi.fn();
    const doc = makeAuxDoc("wiki", "arch/overview");
    const el = render({ docs: [doc], onToggleDoc, onCloseDoc });
    const buttons = Array.from(el.querySelectorAll("button")) as HTMLElement[];
    expect(buttons.some((b) => b.textContent === "✕")).toBe(false);

    act(() => buttons[0].click());
    expect(onToggleDoc).toHaveBeenCalledWith(doc.id);
    expect(onCloseDoc).not.toHaveBeenCalled();
  });

  // The rail is too narrow to read in: an expanded doc keeps its row in the
  // rail, marked open, and is read in the floating reader beside it — never in
  // the drawer, which used to carry it under a duplicate copy of this name.
  it("reads the expanded doc in the floating reader, keeping its row in the rail", () => {
    const doc = makeAuxDoc("wiki", "arch/overview");
    const el = render({ docs: [doc], expandedId: doc.id });

    const row = el.querySelector("[aria-expanded]") as HTMLElement;
    expect(row.getAttribute("aria-expanded")).toBe("true");
    expect(el.querySelector("[class*='doc_card_expanded']")).toBeNull();

    const reader = readerHost!.querySelector("[class*='doc_card_expanded']") as HTMLElement;
    expect(reader.querySelector('[role="separator"]')).not.toBeNull();
    // WikiTabPane mounts inside the reader rather than in a drawer.
    expect(reader.querySelector("[class*='aux_doc_pane']")).not.toBeNull();
  });

  // An expanded doc card's header is the reader's own AuxDocBar. It used to be
  // a strip *above* that bar, which printed the doc's name twice in a row. (An
  // agent card keeps its strip — a transcript has no header to borrow.)
  it("prints the expanded doc's name once, not in a strip of its own", () => {
    const doc = makeAuxDoc("wiki", "arch/overview");
    const el = render({ docs: [doc], expandedId: doc.id });

    expect(el.querySelector("[class*='doc_card_head']")).toBeNull();
    expect(readerHost!.querySelector("[class*='doc_card_head']")).toBeNull();
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

  // Hiding the rail is offered from the thing you want gone, not only from
  // across the header.
  it("puts hiding the rail on a card's menu", () => {
    const el = render({ docs: [makeAuxDoc("file", "/repo/a.rs")] });
    const chip = el.querySelector("[class*='doc_card']") as HTMLElement;

    act(() =>
      void chip.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true })),
    );
    const labels = Array.from(document.body.querySelectorAll("[class*='menu'] button")).map(
      (b) => b.textContent ?? "",
    );

    expect(labels.some((l) => /辅助栏|side rail/i.test(l))).toBe(true);
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

  it("gives the expanded subagent reader a width grip and a way out to its page", () => {
    const onOpenAgent = vi.fn();
    const a = agent("sub-1", "Trace the watcher");
    const el = render({ agents: [a], expandedId: agentCardId("sub-1"), onOpenAgent });
    // The row stays in the rail; the transcript is read beside it.
    expect(el.textContent).toContain("Trace the watcher");
    const card = readerHost!.querySelector("[class*='doc_card_expanded']") as HTMLElement;

    expect(card.querySelector('[role="separator"]')).not.toBeNull();
    // The transcript pane mounts inside the card: grip + head + pane.
    expect(card.childElementCount).toBeGreaterThan(2);

    const goto = card.querySelector(
      `[aria-label="${i18n.t("detail.agent_card_goto")}"]`,
    ) as HTMLElement;
    act(() => goto.click());
    expect(onOpenAgent).toHaveBeenCalledWith(a);
  });

  // The ✕ only closes an open agent's reader; a collapsed row has none, so
  // nothing in the rail can hide a live agent.
  it("offers the ✕ only on an expanded agent card", () => {
    const onCloseAgent = vi.fn();
    const live = agent("sub-1", "Trace the watcher");
    const collapsed = render({ agents: [live], onCloseAgent });
    expect([...collapsed.querySelectorAll("button")].some((b) => b.textContent === "✕")).toBe(false);

    const open = render({ agents: [live], onCloseAgent, expandedId: agentCardId(live.id) });
    const close = [...open.querySelectorAll("button")].find((b) => b.textContent === "✕");
    expect(close).toBeTruthy();
    act(() => close!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onCloseAgent).toHaveBeenCalledWith(live);
  });
});
