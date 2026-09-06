// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DecisionHistoryRecord, SessionInfo } from "../types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

import "../i18n";
import { withCodexDecisionHistory } from "./codexDecision";
import { MessageList } from "./MessageList";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(Element.prototype as unknown as { scrollIntoView: () => void }).scrollIntoView = () => {};

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

describe("MessageList Codex decision history", () => {
  it("renders an answered historical fleet ask as one standalone decision card", async () => {
    const record: DecisionHistoryRecord = {
      kind: "fleet-ask",
      id: "ask-1",
      sessionId: "codex-session",
      workspaceName: "fleet",
      requestedAt: "2026-09-06T12:00:01.000Z",
      resolvedAt: "2026-09-06T12:00:02.000Z",
      outcome: "answered",
      questions: [{
        header: "选择",
        question: "采用哪种方案？",
        multiSelect: false,
        options: [{ label: "方案 A", description: "最小改动" }],
      }],
      answers: { "采用哪种方案？": "方案 A" },
    };
    const session = { id: "codex-session", agentSource: "codex" } as SessionInfo;
    const messages = withCodexDecisionHistory(session, [], [record]);

    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(createElement(MessageList, {
        messages,
        decisionRecords: [record],
        isLoading: false,
      }));
    });

    const buttons = [...container!.querySelectorAll("button")];
    const cardHeaders = buttons.filter((button) =>
      (button.textContent ?? "").includes("采用哪种方案？"),
    );
    expect(cardHeaders).toHaveLength(1);
    expect(cardHeaders[0].textContent).toContain("方案 A");
  });
});
