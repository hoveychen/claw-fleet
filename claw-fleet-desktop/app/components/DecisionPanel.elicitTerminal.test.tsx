// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ElicitationDecision, ElicitationRequest } from "../types";

const invoke = vi.fn(async () => null);
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...(a as [])) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }));

import "../i18n";
import { DecisionCard } from "./DecisionPanel";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(Element.prototype as unknown as { scrollIntoView: () => void }).scrollIntoView = () => {};
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  invoke.mockClear();
});

function request(overrides: Partial<ElicitationRequest> = {}): ElicitationRequest {
  return {
    id: "card-1",
    sessionId: "sess-1",
    workspaceName: "claw-fleet",
    questions: [{
      question: "任务已完成：改完了。",
      header: "任务完成",
      multiSelect: false,
      options: [
        { label: "收到", description: "确认收到" },
        { label: "继续", description: "让会话继续推进" },
      ],
    }],
    timestamp: "2026-09-19T12:00:00.000Z",
    ...overrides,
  } as ElicitationRequest;
}

function decision(req: ElicitationRequest): ElicitationDecision {
  return {
    kind: "elicitation",
    id: req.id,
    request: req,
    step: 0,
    selections: {},
    customAnswers: {},
    multiSelectOverrides: {},
    attachments: {},
    arrivedAt: Date.now(),
  };
}

function render(req: ElicitationRequest) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(createElement(DecisionCard, { decision: decision(req) })));
}

function terminalButton(): HTMLButtonElement | undefined {
  return [...(container?.querySelectorAll("button") ?? [])].find((b) =>
    /结束任务|Finish task/.test(b.textContent ?? ""),
  ) as HTMLButtonElement | undefined;
}

describe("elicitation terminal button", () => {
  it("offers it on the waiting-for-input card and reports the task complete", async () => {
    render(request({ turnCompletion: true }));
    const btn = terminalButton();
    expect(btn).toBeTruthy();

    await act(async () => { btn!.click(); });
    expect(invoke).toHaveBeenCalledWith("respond_to_elicitation", {
      id: "card-1",
      declined: true,
      answers: {},
      taskOutcome: "completed",
    });
  });

  it("leaves a mid-turn AskUserQuestion card alone — an agent is blocked on the answer", () => {
    render(request());
    expect(terminalButton()).toBeUndefined();
  });
});
