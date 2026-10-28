// @vitest-environment jsdom
// The card review asks: "steelman" beside each option and "what would flip it"
// on the question. A press must not send anything — it opens the card's ask bar
// with the preset preselected, and only the bar's send button posts the ask.
// The steelman quote names exactly one option so the host's answer digest can
// say which option the boss looked at.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ExplainRecord, SessionInfo } from "../generated/types";
import type { FleetAskDecision, FleetAskRequest } from "../types";

// Records `list_explanations` hands back for the card; empty by default.
let stored: ExplainRecord[] = [];
const invoke = vi.fn(async (cmd: string, _args?: unknown) => {
  if (cmd === "explain_selection") return running;
  if (cmd === "list_explanations") return stored;
  return null;
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...(a as [string, unknown])) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

await import("../i18n");
const { DecisionCard } = await import("./DecisionPanel");
const { useDecisionStore, useSessionsStore, useUIStore } = await import("../store");

const running: ExplainRecord = {
  id: "exp-1",
  sessionId: "sess-1",
  source: "claude-code",
  createdMs: 1,
  updatedMs: 1,
  preset: "explain",
  quote: "灰度到 5%",
  question: "这段话是什么意思？",
  thread: [],
  status: "running",
  text: "",
  inputTokens: 0,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheCreationTokens: 0,
  durationMs: 0,
} as unknown as ExplainRecord;

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(Element.prototype as unknown as { scrollIntoView: () => void }).scrollIntoView = () => {};
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

function request(): FleetAskRequest {
  return {
    id: "card-1",
    sessionId: "sess-1",
    workspaceName: "claw-fleet",
    questions: [{
      question: "上线计划：先 [?灰度到 5%]，再全量。",
      header: "上线",
      multiSelect: false,
      options: [
        { label: "照做", description: "按这个节奏上线" },
        { label: "直接全量", description: "跳过灰度" },
      ],
    }],
    timestamp: "2026-09-21T12:00:00.000Z",
  } as unknown as FleetAskRequest;
}

function decision(): FleetAskDecision {
  return {
    kind: "fleet-ask",
    id: "card-1",
    request: request(),
    step: 0,
    selections: {},
    customAnswers: {},
    multiSelectOverrides: {},
    attachments: {},
    formValues: {},
    arrivedAt: Date.now(),
  } as unknown as FleetAskDecision;
}

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  useSessionsStore.setState({
    sessions: [{
      id: "sess-1",
      workspacePath: "/Users/foo/repo",
      workspaceName: "repo",
      jsonlPath: "/Users/foo/.claude/projects/repo/sess-1.jsonl",
      lastActivityMs: 1,
      tokenSpeed: 0,
    } as unknown as SessionInfo],
  });
  useDecisionStore.setState({ decisions: [decision()] });
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  window.getSelection()?.removeAllRanges();
  invoke.mockClear();
  stored = [];
  useDecisionStore.setState({ decisions: [] });
});

function mount(ui: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(ui));
  return container;
}

/** Let the bar's effects and any queued animation frame run. */
async function settle() {
  await act(async () => {
    await new Promise<void>((r) => requestAnimationFrame(() => r()));
  });
}

function askCalls() {
  return invoke.mock.calls.filter(([cmd]) => cmd === "explain_selection");
}

function bar(el: HTMLElement) {
  return el.querySelector<HTMLElement>("[data-testid='selection-toolbar']");
}

/** A full press: mousedown/mouseup like a real click (the bar listens to both). */
async function press(btn: HTMLElement) {
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    btn.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    btn.click();
  });
  await settle();
}

describe("card review asks (steelman / flip conditions)", () => {
  it("steelman preselects on the bar and sends only on confirm, quoting one option", async () => {
    const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
    await act(async () => {});
    expect(bar(el), "no bar before any press").toBeNull();

    const buttons = el.querySelectorAll<HTMLButtonElement>("[data-testid='card-steelman-btn']");
    expect(buttons.length, "one steelman button per option").toBe(2);
    await press(buttons[1]);

    const b = bar(el);
    expect(b, "the press opens the ask bar").not.toBeNull();
    const chosen = b!.querySelector("button[aria-pressed='true']");
    expect(chosen?.getAttribute("data-preset")).toBe("steelman");
    expect(b!.textContent, "the caption names the option").toContain("直接全量");
    expect(askCalls().length, "a press alone must not ask").toBe(0);

    await press(b!.querySelector<HTMLElement>("[data-testid='selection-toolbar-send']")!);

    expect(askCalls().length).toBe(1);
    const req = (askCalls()[0][1] as { request: { preset: string; quote: string; question?: string } }).request;
    expect(req.preset).toBe("steelman");
    expect(req.quote).toBe("上线计划：先 [?灰度到 5%]，再全量。\n\n【被追问的选项】直接全量 — 跳过灰度");
    expect(req.quote.includes("照做"), "only the pressed option's label is quoted").toBe(false);
    expect(req.question).toBeUndefined();
  });

  it("flip conditions quotes the question, and another preset can be chosen before sending", async () => {
    const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
    await act(async () => {});

    await press(el.querySelector<HTMLElement>("[data-testid='card-flip-btn']")!);
    const b = bar(el)!;
    expect(b.querySelector("button[aria-pressed='true']")?.getAttribute("data-preset")).toBe("flip_conditions");

    // Switching the preset on a primed bar re-chooses, it does not send.
    const why = b.querySelector<HTMLButtonElement>("button[data-preset='rationale']")!;
    await press(why);
    expect(askCalls().length).toBe(0);
    expect(bar(el)!.querySelector("button[aria-pressed='true']")?.getAttribute("data-preset")).toBe("rationale");

    await press(bar(el)!.querySelector<HTMLElement>("[data-testid='selection-toolbar-send']")!);
    const req = (askCalls()[0][1] as { request: { preset: string; quote: string } }).request;
    expect(req.preset).toBe("rationale");
    expect(req.quote).toBe("上线计划：先 [?灰度到 5%]，再全量。");
  });

  it("opens on the first press when the priming selectionchange beats React's commit (WebKit)", async () => {
    // In WKWebView the `selectionchange` from the bar's own removeAllRanges()
    // arrives after the prime effect queued its state but before React
    // committed it; jsdom never fires one. It must not close the bar.
    const original = Selection.prototype.removeAllRanges;
    const spy = vi.spyOn(Selection.prototype, "removeAllRanges").mockImplementation(function (this: Selection) {
      original.call(this);
      queueMicrotask(() => document.dispatchEvent(new Event("selectionchange")));
    });
    try {
      const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
      await act(async () => {});

      await press(el.querySelector<HTMLElement>("[data-testid='card-flip-btn']")!);
      expect(bar(el), "flip opens the bar on the first press").not.toBeNull();
      expect(bar(el)!.querySelector("button[aria-pressed='true']")?.getAttribute("data-preset")).toBe("flip_conditions");
    } finally {
      spy.mockRestore();
    }
  });

  it("offers both new presets on a plain selection bar too", async () => {
    const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
    const mark = el.querySelector<HTMLElement>("[data-explain-quote][role='button']")!;
    act(() => mark.click());
    await settle();
    expect(bar(el)!.querySelector("[data-preset='steelman']")).not.toBeNull();
    expect(bar(el)!.querySelector("[data-preset='flip_conditions']")).not.toBeNull();
    expect(bar(el)!.querySelector("[data-testid='selection-toolbar-send']"), "a selection bar sends per preset").toBeNull();
  });
});
