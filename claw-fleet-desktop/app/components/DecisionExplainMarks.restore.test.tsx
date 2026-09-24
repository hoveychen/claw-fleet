// @vitest-environment jsdom
// Side questions asked from a card lived in the panel hook's state alone and
// were wiped on every card switch, so switching to another question and back
// showed an empty panel even though the records were on disk (the session rail
// still listed them). The hook now re-reads the store for the active card.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ExplainRecord } from "../generated/types";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }));

const { useDecisionExplainMarks, isCardExplain } = await import("./DecisionExplainMarks");
type Explain = ReturnType<typeof useDecisionExplainMarks>;

const CARD_TS = "2026-09-23T10:00:00Z";
const CARD_MS = Date.parse(CARD_TS);

function rec(id: string, over: Partial<ExplainRecord> = {}): ExplainRecord {
  return {
    id,
    sessionId: "sess-a",
    source: "claude-code",
    createdMs: CARD_MS + 1000,
    updatedMs: CARD_MS + 1000,
    preset: "explain",
    quote: "灰度到 5%",
    question: `q-${id}`,
    thread: [],
    status: "done",
    text: `a-${id}`,
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    durationMs: 0,
    dismissed: false,
    ...over,
  };
}

const store: Record<string, ExplainRecord[]> = {};
let host: HTMLDivElement;
let root: Root;
let latest: Explain;

function Probe({ sessionId, ts }: { sessionId: string | null; ts: string }) {
  latest = useDecisionExplainMarks(sessionId, ts);
  return null;
}

async function show(sessionId: string | null, ts = CARD_TS) {
  await act(async () => {
    root.render(createElement(Probe, { sessionId, ts }));
  });
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  for (const k of Object.keys(store)) delete store[k];
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args: { sessionId: string }) => {
    if (cmd === "list_explanations") return store[args.sessionId] ?? [];
    return null;
  });
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

describe("card side questions survive a card switch", () => {
  it("hands the stored answers back when the card is shown again", async () => {
    store["sess-a"] = [rec("r1")];
    store["sess-b"] = [];
    await show("sess-a");
    expect(latest.answers.map((r) => r.id)).toEqual(["r1"]);
    await show("sess-b");
    expect(latest.answers).toEqual([]);
    await show("sess-a");
    expect(latest.answers.map((r) => r.id)).toEqual(["r1"]);
  });

  it("persists a dismissal so the answer stays gone after switching back", async () => {
    store["sess-a"] = [rec("r1")];
    await show("sess-a");
    act(() => latest.dismiss("r1"));
    expect(latest.answers).toEqual([]);
    expect(invoke).toHaveBeenCalledWith("dismiss_explanation", { sessionId: "sess-a", id: "r1", dismissed: true });
  });
});

describe("isCardExplain", () => {
  it("keeps only undismissed, unanchored records asked since the card was posted", () => {
    expect(isCardExplain(rec("ok"), CARD_MS)).toBe(true);
    expect(isCardExplain(rec("old", { createdMs: CARD_MS - 1 }), CARD_MS)).toBe(false);
    expect(isCardExplain(rec("gone", { dismissed: true }), CARD_MS)).toBe(false);
    expect(isCardExplain(rec("rail", { anchor: { msgIdx: 3 } }), CARD_MS)).toBe(false);
  });
});
