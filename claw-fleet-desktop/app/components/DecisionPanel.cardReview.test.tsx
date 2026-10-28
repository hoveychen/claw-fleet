// @vitest-environment jsdom
// The card review asks: "steelman" beside each option and "what would flip it"
// on the question. A press sends the ask at once (no ask bar in between), the
// answer is headed by what was asked, and the button waits while its ask is
// being answered. The steelman quote names exactly one option so the host's
// answer digest can say which option the boss looked at. Plus the selection
// bar, which must stay inside a narrow card.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ExplainRecord, SessionInfo } from "../generated/types";
import type { FleetAskDecision, FleetAskRequest } from "../types";

// Records `list_explanations` hands back for the card; empty by default.
let stored: ExplainRecord[] = [];
const invoke = vi.fn(async (cmd: string, args?: unknown) => {
  // The host echoes the request's preset and quote back on the running record.
  if (cmd === "explain_selection") {
    const req = (args as { request: { preset: string; quote: string } }).request;
    return { ...running, preset: req.preset, quote: req.quote };
  }
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

async function press(btn: HTMLElement) {
  await act(async () => {
    btn.click();
  });
  await settle();
}

function request0() {
  return askCalls()[0][1] as { request: { preset: string; quote: string; question?: string } };
}

describe("card review asks (steelman / flip conditions)", () => {
  it("steelman sends at once, quoting one option, and heads the answer with it", async () => {
    const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
    await act(async () => {});

    const buttons = el.querySelectorAll<HTMLButtonElement>("[data-testid='card-steelman-btn']");
    expect(buttons.length, "one steelman button per option").toBe(2);
    await press(buttons[1]);

    expect(bar(el), "no ask bar in between").toBeNull();
    expect(askCalls().length, "the press itself asks").toBe(1);
    const req = request0().request;
    expect(req.preset).toBe("steelman");
    expect(req.quote).toBe("上线计划：先 [?灰度到 5%]，再全量。\n\n【被追问的选项】直接全量 — 跳过灰度");
    expect(req.quote.includes("照做"), "only the pressed option's label is quoted").toBe(false);
    expect(req.question).toBeUndefined();

    const thread = el.querySelector<HTMLElement>("[data-testid='decision-explain-thread']");
    expect(thread, "the answer lands under the question").not.toBeNull();
    // The test locale is English: "Steelman" is detail.explain_preset_steelman.
    expect(thread!.textContent).toContain("Steelman · 直接全量");
  });

  it("flip conditions sends the question at once, headed by the ask", async () => {
    const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
    await act(async () => {});

    await press(el.querySelector<HTMLElement>("[data-testid='card-flip-btn']")!);
    expect(askCalls().length).toBe(1);
    const req = request0().request;
    expect(req.preset).toBe("flip_conditions");
    expect(req.quote).toBe("上线计划：先 [?灰度到 5%]，再全量。");
    const head = el.querySelector<HTMLElement>("[data-testid='decision-explain-thread']")!.textContent ?? "";
    expect(head).toContain("What would flip it");
    expect(head.includes("上线计划"), "the heading is not the raw quote").toBe(false);
  });

  it("a button waits while its own ask is being answered; the others do not", async () => {
    const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
    await act(async () => {});

    const steelmen = () => el.querySelectorAll<HTMLButtonElement>("[data-testid='card-steelman-btn']");
    await press(steelmen()[1]);
    expect(steelmen()[1].disabled, "the pressed one is busy").toBe(true);
    expect(steelmen()[1].getAttribute("aria-busy")).toBe("true");
    expect(steelmen()[0].disabled, "another option can still be asked about").toBe(false);

    await press(steelmen()[1]);
    expect(askCalls().length, "a second press does not fork again").toBe(1);
  });

  it("offers both new presets on a plain selection bar, which sends per preset", async () => {
    const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
    const mark = el.querySelector<HTMLElement>("[data-explain-quote][role='button']")!;
    act(() => mark.click());
    await settle();
    expect(bar(el)!.querySelector("[data-preset='steelman']")).not.toBeNull();
    expect(bar(el)!.querySelector("[data-preset='flip_conditions']")).not.toBeNull();
    await press(bar(el)!.querySelector<HTMLElement>("[data-preset='steelman']")!);
    expect(askCalls().length).toBe(1);
    expect(request0().request.quote).toBe("灰度到 5%");
  });

  it("keeps the selection bar inside a card narrower than its row of presets", async () => {
    // jsdom has no layout: give the question body a 300px width and the bar a
    // natural 420px one, as a narrow dock card would have.
    const widths = vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockImplementation(function (this: HTMLElement) {
      return this.dataset.testid === "selection-toolbar" ? 420 : 0;
    });
    const client = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (this: HTMLElement) {
      return this.dataset.role === "assistant" ? 300 : 0;
    });
    try {
      const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));
      const mark = el.querySelector<HTMLElement>("[data-explain-quote][role='button']")!;
      act(() => mark.click());
      await settle();
      const b = bar(el)!;
      expect(b.style.visibility, "placed once measured").not.toBe("hidden");
      expect(b.style.maxWidth, "held to the pane less its edges").toBe("284px");
      const left = parseFloat(b.style.left);
      expect(left).toBeGreaterThanOrEqual(8);
      expect(left + 284, "right edge inside the pane").toBeLessThanOrEqual(300 - 8);
    } finally {
      widths.mockRestore();
      client.mockRestore();
    }
  });
});
