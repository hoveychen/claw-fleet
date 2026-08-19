// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import "../i18n";
import { MessageList } from "./MessageList";
import type { RawMessage } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const ERROR_TEXT =
  "Your access token could not be refreshed because your refresh token was already used. Please log out and sign in again.";

/** The exact shape `normalize_messages` emits for a codex turn that died
 *  before replying — see `codex_turn_error_text` and its Rust tests. */
const transcript: RawMessage[] = [
  {
    type: "user",
    uuid: "u1",
    timestamp: "2026-08-19T18:20:02.155Z",
    message: { role: "user", content: "why is the graph mocked?" },
  },
  {
    type: "assistant",
    uuid: "e1",
    isTurnError: true,
    timestamp: "2026-08-19T18:20:02.752Z",
    message: {
      role: "assistant",
      content: [{ type: "text", text: ERROR_TEXT }],
      stop_reason: "end_turn",
    },
  },
] as unknown as RawMessage[];

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function mount(node: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root.render(node));
}

describe("MessageList — failed turn", () => {
  it("shows the failure reason instead of an endless spinner", () => {
    mount(<MessageList messages={transcript} isLoading={false} status="waitingInput" />);

    expect(container.querySelector('[data-testid="turn-error"]')).not.toBeNull();
    expect(container.textContent).toContain(ERROR_TEXT);
    // The bug: the pane sat on 「处理中…」 forever with the reason nowhere.
    expect(container.textContent).not.toContain("Processing");
    expect(container.textContent).not.toContain("处理中");
  });

  it("stops spinning on a dead turn that left no record at all", () => {
    // The harness-crash case: no terminal record ever hit the rollout, so the
    // transcript ends on the prompt. Nothing is running (the scanner says so),
    // and the prompt is minutes old — this must not read as "about to run".
    mount(<MessageList messages={[transcript[0]]} isLoading={false} status="waitingInput" />);
    expect(container.textContent).not.toContain("Processing");
    expect(container.textContent).not.toContain("处理中");
  });

  it("still spins while the agent is genuinely working", () => {
    mount(
      <MessageList
        messages={[transcript[0]]}
        isLoading={false}
        status="thinking"
      />,
    );
    expect(container.querySelector('[data-testid="turn-error"]')).toBeNull();
    expect(container.textContent).toMatch(/Thinking|思考/);
  });
});
