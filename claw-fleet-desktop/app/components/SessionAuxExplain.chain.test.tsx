// @vitest-environment jsdom
// A rail side-question card is one *chain*, not one record. A follow-up is its
// own record (the fork is never resumed), so a card per record spent one of
// the rail's ten slots per turn and stacked the turns above the questions they
// answered. The grouping itself is pinned in explainThreads.test.ts.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ExplainRecord } from "../generated/types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }));
const writeText = vi.fn(async () => {});
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: (s: string) => writeText(s) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

await import("../i18n");
const { SessionAuxExplain, chainAnswerText } = await import("./SessionAuxExplain");

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

const CHAIN = [rec("a", [], "done", 1), rec("b", ["a"], "done", 2)];

let host: HTMLDivElement;
let root: Root;
const onFollowUp = vi.fn();
const onClose = vi.fn();

function render(records: ExplainRecord[], isOpen = true) {
  act(() => {
    root.render(
      createElement(SessionAuxExplain, {
        records,
        isOpen,
        onToggle: vi.fn(),
        onClose,
        onLocate: vi.fn(),
        onFollowUp,
        onGripDown: vi.fn(),
        onHideRail: vi.fn(),
      }),
    );
  });
}

beforeEach(() => {
  onFollowUp.mockReset();
  onClose.mockReset();
  writeText.mockReset();
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

describe("SessionAuxExplain as a chain card", () => {
  it("shows every turn in asking order under one quote", () => {
    render(CHAIN);
    const text = host.textContent ?? "";
    expect(text.indexOf("a-a")).toBeLessThan(text.indexOf("a-b"));
    expect(host.querySelectorAll("form")).toHaveLength(1);
    // The passage is quoted once for the chain, not once per turn.
    expect(text.split("灰度到 5%").length - 1).toBe(1);
  });

  it("withholds the follow-up box while the chain's latest turn is forking", () => {
    render([rec("a", [], "done", 1), rec("b", ["a"], "running", 2)]);
    expect(host.querySelectorAll("form")).toHaveLength(0);
  });

  it("copies the whole conversation, not just one turn", () => {
    const copied = chainAnswerText(CHAIN);
    expect(copied).toContain("q-a");
    expect(copied).toContain("a-a");
    expect(copied).toContain("a-b");
    // A turn still forking has no answer yet and contributes nothing.
    expect(chainAnswerText([rec("a", [], "done", 1), rec("b", ["a"], "running", 2)])).toBe(
      "q-a\n\na-a",
    );
  });

  it("counts the chain's turns on the collapsed chip", () => {
    render(CHAIN, false);
    expect(host.textContent ?? "").toMatch(/2/);
  });
});
