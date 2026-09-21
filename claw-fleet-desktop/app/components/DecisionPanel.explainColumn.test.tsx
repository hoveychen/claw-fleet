// @vitest-environment jsdom
// Where a card's side questions land. Asking from inside a card used to append
// the answer to the question body, which shares its scroller with the prose
// while the footer takes the height it wants — on a long card the answer was a
// clipped sliver below the fold and the ask looked like it had done nothing.
// In the panel the answer belongs to the side column; only a card rendered
// outside the panel keeps it under the question.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ExplainRecord, SessionInfo } from "../generated/types";
import type { FleetAskDecision, FleetAskRequest } from "../types";

const invoke = vi.fn(async (cmd: string) => {
  if (cmd === "explain_selection") return running;
  return null;
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...(a as [string])) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

await import("../i18n");
const { DecisionPanel, DecisionCard } = await import("./DecisionPanel");
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
  useDecisionStore.setState({ decisions: [decision()], activeDecisionId: "card-1" });
  useUIStore.getState().setDecisionPanelCollapsed(false);
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  window.getSelection()?.removeAllRanges();
  invoke.mockClear();
  useDecisionStore.setState({ decisions: [], activeDecisionId: null });
});

function mount(ui: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(ui));
  return container;
}

/** The bar's `read` runs on the next animation frame; wait it out. */
async function nextFrame() {
  await act(async () => {
    await new Promise<void>((r) => requestAnimationFrame(() => r()));
  });
}

/** Click the card's `[?…]` mark, then the bar's first preset (解释). */
async function askAboutTheMark(el: HTMLElement) {
  const mark = el.querySelector<HTMLElement>("[data-explain-quote][role='button']");
  expect(mark, "the card's [?…] mark should be clickable").not.toBeNull();
  act(() => mark!.click());
  await nextFrame();
  const preset = el.querySelector<HTMLButtonElement>("[data-testid='selection-toolbar'] button");
  expect(preset, "the ask bar should be up").not.toBeNull();
  await act(async () => { preset!.click(); });
}

describe("side questions asked from a decision card", () => {
  it("answers in the panel's side column, not at the tail of the question", async () => {
    const el = mount(createElement(DecisionPanel));
    expect(el.querySelector("[data-testid='decision-explain-column']")).toBeNull();

    await askAboutTheMark(el);

    expect(invoke).toHaveBeenCalledWith("explain_selection", {
      request: expect.objectContaining({ quote: "灰度到 5%", preset: "explain", sessionId: "sess-1" }),
    });
    const column = el.querySelector("[data-testid='decision-explain-column']");
    expect(column, "the column should open itself on the first answer").not.toBeNull();
    // The answer lives in the column and nowhere else — in particular not
    // inside the card's own scroller, where it used to be clipped.
    const answers = el.querySelectorAll("[data-testid='decision-explain-answers']");
    expect(answers).toHaveLength(1);
    expect(column!.contains(answers[0])).toBe(true);
    expect(answers[0].textContent).toContain("灰度到 5%");
  });

  it("keeps the answer under the question on a card rendered outside the panel", async () => {
    const el = mount(createElement(DecisionCard, { decision: decision(), compact: true }));

    await askAboutTheMark(el);

    expect(el.querySelector("[data-testid='decision-explain-column']")).toBeNull();
    const answers = el.querySelector("[data-testid='decision-explain-answers']");
    expect(answers, "the inline card has no column to send it to").not.toBeNull();
    expect(answers!.textContent).toContain("灰度到 5%");
  });
});
