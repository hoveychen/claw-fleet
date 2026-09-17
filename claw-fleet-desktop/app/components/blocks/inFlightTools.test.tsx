// @vitest-environment jsdom
// The card's "this tool is running" affordance. Both halves of the bug the user hit
// are covered: a finalised `tool_use` record whose Bash has not returned yet
// (used to render as a silent, finished-looking card), and a background shell
// whose turn ended while the command kept running.
import { afterEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import "../../i18n";
import { ToolUseBlock } from "./ToolUseBlock";
import { InFlightToolsContext, inFlightToolIds } from "./inFlightTools";
import type { RawMessage, ToolResultBlock } from "../../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function assistant(stopReason: string | null, ids: string[]): RawMessage {
  return {
    type: "assistant",
    message: {
      stop_reason: stopReason,
      content: ids.map((id) => ({ type: "tool_use", id, name: "Bash", input: { command: "sleep 600" } })),
    },
  } as unknown as RawMessage;
}

describe("inFlightToolIds", () => {
  it("claims a finalised tool_use whose result has not landed", () => {
    // The exact window the old `isPartial` check missed: stop_reason is already
    // "tool_use", so nothing is streaming, but the tool is still executing.
    const ids = inFlightToolIds([assistant("tool_use", ["t1"])], new Map(), true);
    expect([...ids]).toEqual(["t1"]);
  });

  it("stays empty when the scanner says the agent is not working", () => {
    // A killed turn leaves the same shape behind forever — never label it running.
    expect(inFlightToolIds([assistant("tool_use", ["t1"])], new Map(), false).size).toBe(0);
  });

  it("ignores calls that already have a result", () => {
    const results = new Map<string, ToolResultBlock>([
      ["t1", { type: "tool_result", tool_use_id: "t1", content: "done" } as ToolResultBlock],
    ]);
    const ids = inFlightToolIds([assistant("tool_use", ["t1", "t2"])], results, true);
    expect([...ids]).toEqual(["t2"]);
  });

  it("only reads the newest assistant record", () => {
    // An older record missing its result was trimmed or windowed away, not live.
    const msgs = [assistant("tool_use", ["old"]), assistant("end_turn", [])];
    expect(inFlightToolIds(msgs, new Map(), true).size).toBe(0);
  });
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function mount(el: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(el));
}

describe("ToolUseBlock running affordance", () => {
  const block = { type: "tool_use", id: "t1", name: "Bash", input: { command: "cargo test" } };

  it("spins for an in-flight call even though its record is no longer partial", () => {
    mount(
      createElement(
        InFlightToolsContext.Provider,
        { value: new Set(["t1"]) },
        createElement(ToolUseBlock, { block, isPartial: false } as never),
      ),
    );
    expect(container!.textContent).toContain("⟳");
  });

  it("shows nothing extra once the call is no longer in flight", () => {
    mount(
      createElement(
        InFlightToolsContext.Provider,
        { value: new Set<string>() },
        createElement(ToolUseBlock, { block, isPartial: false } as never),
      ),
    );
    expect(container!.textContent).not.toContain("⟳");
  });

  it("labels a background shell, whose result lands instantly but keeps running", () => {
    const bg = { ...block, input: { command: "cargo test", run_in_background: true } };
    const result = { type: "tool_result", tool_use_id: "t1", content: "Running in background" };
    mount(createElement(ToolUseBlock, { block: bg, result, isPartial: false } as never));
    expect(container!.textContent).toMatch(/后台|background/i);
  });
});
