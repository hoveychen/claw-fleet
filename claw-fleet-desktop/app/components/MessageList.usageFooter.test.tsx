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

/** A mid-turn prose record: Claude Code flushes each content block as its own
 *  record, so the sentence before a tool call carries no usage of its own. */
function prose(text: string): RawMessage {
  return {
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text }] },
  } as unknown as RawMessage;
}

/** The turn's final record — the one usage is attributed to. */
function toolWithUsage(tool: string): RawMessage {
  return {
    type: "assistant",
    message: {
      role: "assistant",
      content: [{ type: "tool_use", id: `t-${tool}`, name: tool, input: {} }],
      usage: { input_tokens: 100, output_tokens: 20 },
    },
  } as unknown as RawMessage;
}

function render(messages: RawMessage[]) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root.render(<MessageList messages={messages} isLoading={false} />));
}

/** Every assistant footer's class list, in document order. */
function footers(): string[] {
  return [...container.querySelectorAll("div")]
    .map((el) => el.className)
    .filter((c) => /\busage\b|usage_/.test(c) && !/row_actions/.test(c));
}

describe("assistant usage footer", () => {
  // The bug: 231b0a32 made every record with prose render a footer so the
  // read/copy pair always had a home, but usage is attributed only to a turn's
  // last record. The mid-turn "say a sentence, then call a tool" record —
  // extremely common — therefore drew an empty 26px line under most prose.
  it("collapses the footer on a prose record that has no counts", () => {
    render([prose("构建还在跑。等它。"), toolWithUsage("Bash")]);
    const [proseFooter, tailFooter] = footers();
    expect(proseFooter).toMatch(/usage_bare/);
    expect(tailFooter).toBeDefined();
    expect(tailFooter).not.toMatch(/usage_bare/);
  });

  it("keeps a full footer when the record carries its own counts", () => {
    render([{
      type: "assistant",
      message: {
        role: "assistant",
        content: [{ type: "text", text: "done" }],
        usage: { input_tokens: 100, output_tokens: 20 },
      },
    } as unknown as RawMessage]);
    expect(footers()[0]).not.toMatch(/usage_bare/);
  });
});
