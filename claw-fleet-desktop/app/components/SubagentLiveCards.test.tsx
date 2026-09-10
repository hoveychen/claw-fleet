// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import "../i18n";
import { agentCardId } from "../detailAux";
import type { SessionInfo } from "../types";
import { SubagentLiveCards } from "./SubagentLiveCards";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  // The menu is portalled to the body, so it outlives the container.
  document.body.querySelectorAll("[class*='menu']").forEach((n) => n.remove());
});

function agent(over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id: "a4f1-9c",
    aiTitle: "Trace the watcher",
    agentType: "Explore",
    status: "executing",
    isSubagent: true,
    lastActivityMs: Date.now(),
    createdAtMs: Date.now() - 134_000,
    agentTokenSpeed: 18,
    totalCostUsd: 0.42,
    model: "claude-opus-5",
    effort: "xhigh",
    jsonlPath: "/home/me/.claude/projects/p/a4f1-9c.jsonl",
    ...over,
  } as unknown as SessionInfo;
}

function render(
  a: SessionInfo,
  props: Partial<Parameters<typeof SubagentLiveCards>[0]> = {},
) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <SubagentLiveCards
        agents={[a]}
        expandedId={null}
        onToggle={() => {}}
        onClose={() => {}}
        onGoto={() => {}}
        onGripDown={() => {}}
        renderPane={() => <div data-testid="pane" />}
        {...props}
      />,
    ),
  );
  return container;
}

function rightClick(el: HTMLElement): MouseEvent {
  const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
  act(() => void el.dispatchEvent(ev));
  return ev;
}

describe("SubagentLiveCards", () => {
  // These fields were all on SessionInfo already and none were shown: the card
  // said what an agent *is* and nothing about how it is doing.
  it("shows what the agent was given and how it is doing", () => {
    const el = render(agent());

    expect(el.textContent).toContain("Opus 5");
    expect(el.textContent).toContain("xhigh");
    // Elapsed, not just "last seen" — a healthy agent and a wedged one both
    // read "just now" the moment they print anything.
    expect(el.textContent).toContain("2m 14s");
    expect(el.textContent).toContain("18 tok/s");
    expect(el.textContent).toContain("$0.42");
  });

  it("leaves the spec row out when the scan learned neither model nor effort", () => {
    const el = render(agent({ model: null, effort: null } as Partial<SessionInfo>));

    expect(el.querySelector("[class*='agent_card_spec']")).toBeNull();
  });

  // Unlike the preview, the spec survives expansion: the transcript pane below
  // names no model, so collapsing that line would lose the fact entirely.
  it("keeps the spec row once the transcript is open", () => {
    const a = agent();
    const el = render(a, { expandedId: agentCardId(a.id) });

    expect(el.querySelector("[class*='agent_card_spec']")).not.toBeNull();
    expect(el.querySelector('[data-testid="pane"]')).not.toBeNull();
  });

  it("answers a right-click with the agent's own menu, not the app-wide one", () => {
    const el = render(agent());
    const ev = rightClick(el.querySelector("button") as HTMLElement);

    // preventDefault is what stops contextMenu.ts answering with 设置/关于/退出.
    expect(ev.defaultPrevented).toBe(true);
    const menu = document.body.querySelector("[class*='menu']") as HTMLElement;
    expect(menu.textContent).toContain("a4f1-9c");
    expect(menu.textContent).toContain(".jsonl");
  });

  // A subagent has no signal of its own (StopControl.canControl === !isSubagent)
  // and `pid` is the parent's, so an item labelled "stop this agent" would
  // cancel the parent's whole turn. It must not be offered here.
  it("offers no stop, because a subagent is not ours to signal", () => {
    const el = render(agent());
    rightClick(el.querySelector("button") as HTMLElement);

    const menu = document.body.querySelector("[class*='menu']") as HTMLElement;
    expect(menu.textContent).not.toMatch(/停止|Stop|Interrupt|中断/);
  });

  // Expanding here is the default and leaving is the escape hatch, so the menu
  // has to offer both — and must not confuse them.
  it("separates expanding the card from leaving for the session page", () => {
    const onToggle = vi.fn();
    const onGoto = vi.fn();
    const a = agent();
    const el = render(a, { onToggle, onGoto });
    rightClick(el.querySelector("button") as HTMLElement);
    const items = Array.from(document.body.querySelectorAll("[class*='menu'] button"));

    act(() => (items[0] as HTMLElement).click());
    expect(onToggle).toHaveBeenCalledWith(a);
    expect(onGoto).not.toHaveBeenCalled();

    rightClick(el.querySelector("button") as HTMLElement);
    const again = Array.from(document.body.querySelectorAll("[class*='menu'] button"));
    act(() => (again[1] as HTMLElement).click());
    expect(onGoto).toHaveBeenCalledWith(a);
  });
});
