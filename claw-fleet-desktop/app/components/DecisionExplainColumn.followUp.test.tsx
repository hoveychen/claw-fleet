// @vitest-environment jsdom
// The side column used to be read-only: the only way to ask anything was to
// select prose in the card again, and the request went out with thread: [], so
// the second question knew nothing about the first answer. The column now ends
// each chain with a follow-up box that threads off that chain's last turn.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ExplainRecord } from "../generated/types";
import type { DecisionExplain } from "./DecisionExplainMarks";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }));

await import("../i18n");
const { DecisionExplainColumn } = await import("./DecisionExplainColumn");

function rec(id: string, thread: string[], status: ExplainRecord["status"], createdMs: number): ExplainRecord {
  return {
    id,
    sessionId: "sess-1",
    source: "claude-code",
    createdMs,
    updatedMs: createdMs,
    preset: thread.length > 0 ? "custom" : "explain",
    quote: "灰度到 5%",
    question: `q-${id}`,
    thread,
    status,
    text: status === "done" ? `a-${id}` : "",
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    durationMs: 0,
  };
}

let host: HTMLDivElement;
let root: Root;
const followUp = vi.fn();

function render(answers: ExplainRecord[], over: Partial<DecisionExplain> = {}) {
  const explain: DecisionExplain = {
    enabled: true,
    busy: false,
    ask: vi.fn(),
    followUp,
    answers,
    dismiss: vi.fn(),
    ...over,
  };
  act(() => {
    root.render(createElement(DecisionExplainColumn, { explain }));
  });
}

function forms() {
  return Array.from(host.querySelectorAll("form"));
}

/** React tracks the value property itself, so a plain assignment is invisible
 *  to it — go through the native setter before dispatching. */
function type(input: HTMLInputElement, text: string) {
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
  act(() => {
    setter?.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  followUp.mockReset();
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

describe("DecisionExplainColumn follow-up", () => {
  it("offers exactly one box per chain, at its foot", () => {
    render([rec("a", [], "done", 1), rec("b", ["a"], "done", 2), rec("x", [], "done", 5)]);
    // Two chains ('a'→'b' and 'x'), so two boxes — not one per record.
    expect(forms()).toHaveLength(2);
  });

  it("threads off the chain's last turn, not its root", () => {
    render([rec("a", [], "done", 1), rec("b", ["a"], "done", 2)]);
    const form = forms()[0];
    type(form.querySelector("input") as HTMLInputElement, "那这个阈值怎么定的？");
    act(() => {
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    expect(followUp).toHaveBeenCalledTimes(1);
    expect(followUp.mock.calls[0][0].id).toBe("b");
    expect(followUp.mock.calls[0][1]).toBe("那这个阈值怎么定的？");
  });

  it("stays away until the chain's last answer has settled", () => {
    render([rec("a", [], "done", 1), rec("b", ["a"], "running", 2)]);
    expect(forms()).toHaveLength(0);
  });

  it("ignores a blank question", () => {
    render([rec("a", [], "done", 1)]);
    act(() => {
      forms()[0].dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    expect(followUp).not.toHaveBeenCalled();
  });

  it("does not fire a second ask while one is in flight", () => {
    render([rec("a", [], "done", 1)], { busy: true });
    const form = forms()[0];
    type(form.querySelector("input") as HTMLInputElement, "再问一句");
    act(() => {
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    expect(followUp).not.toHaveBeenCalled();
  });
});
