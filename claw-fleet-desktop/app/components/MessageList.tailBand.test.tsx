// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import "../i18n";
import { MessageList } from "./MessageList";
import type { RawMessage } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root;
let container: HTMLDivElement;

beforeAll(() => { Element.prototype.scrollIntoView = () => {}; });
afterEach(() => { act(() => root.unmount()); container.remove(); });

/** One pure-work assistant record: thinking + a tool call, no prose. */
function work(thinking: string, tool: string): RawMessage {
  return {
    type: "assistant",
    message: {
      role: "assistant",
      content: [
        { type: "thinking", thinking },
        { type: "tool_use", id: `t-${tool}-${thinking}`, name: tool, input: {} },
      ],
    },
  } as unknown as RawMessage;
}

function prose(text: string): RawMessage {
  return {
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text }] },
  } as unknown as RawMessage;
}

function render(messages: RawMessage[], status?: string) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root.render(<MessageList messages={messages} isLoading={false} status={status} />));
}

/** Open/closed reads off the band header's disclosure arrow. */
function arrows(): string[] {
  return [...container.querySelectorAll("button")]
    .map((el) => el.textContent ?? "")
    .filter((t) => t.startsWith("▾") || t.startsWith("▸"))
    .map((t) => t.slice(0, 1));
}

describe("trailing work-run band", () => {
  // The bug: a band that formed while the session status had dropped out of the
  // working set mounted folded, so a running session showed only a rising step
  // count and a newer timestamp with nothing to read.
  it("opens the last band even when the session status is not a working one", () => {
    render([work("Reading the store", "Read"), work("Now the grep", "Grep")]);
    expect(arrows()).toEqual(["▾"]);
  });

  it("leaves earlier bands folded — only the tail opens", () => {
    render([
      work("Reading the store", "Read"),
      work("Now the grep", "Grep"),
      prose("Now wiring the config UI into the product entry."),
      work("Editing the entry", "Edit"),
      work("Running the tests", "Bash"),
    ]);
    expect(arrows()).toEqual(["▸", "▾"]);
  });
});
