// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ExplainRecord } from "../explainApi";
import { useSessionExplains } from "./useSessionExplains";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** The store, as the mocked host serves it: records plus the dismissal set the
 *  host stamps onto them — exactly what `session_explain.rs` does on read. */
const store = {
  records: [] as ExplainRecord[],
  dismissed: new Set<string>(),
};

vi.mock("../explainApi", () => ({
  listExplanations: vi.fn(async (sessionId: string) =>
    store.records
      .filter((r) => r.sessionId === sessionId)
      .map((r) => ({ ...r, dismissed: store.dismissed.has(r.id) })),
  ),
  getExplanation: vi.fn(async () => {
    throw new Error("not polled in these tests");
  }),
  explainSelection: vi.fn(async () => {
    throw new Error("not asked in these tests");
  }),
  dismissExplanation: vi.fn(async (_sessionId: string, id: string, dismissed: boolean) => {
    if (dismissed) store.dismissed.add(id);
    else store.dismissed.delete(id);
  }),
  pollExplanation: vi.fn(async () => {}),
  EXPLAIN_POLL_MS: 500,
}));

function rec(id: string, sessionId: string): ExplainRecord {
  return {
    id,
    sessionId,
    source: "claude-code",
    createdMs: 1,
    updatedMs: 1,
    preset: "explain",
    quote: "q",
    question: "why",
    thread: [],
    status: "done",
    text: "because",
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    durationMs: 0,
    dismissed: false,
  } as ExplainRecord;
}

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  vi.clearAllMocks();
  store.records = [rec("e1", "s1"), rec("e2", "s1")];
  store.dismissed = new Set();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

/** Mounts the hook the way SessionDetail holds it: re-pointed at another
 *  session rather than remounted. */
function mount(sessionId: string) {
  const seen = {
    explains: [] as ExplainRecord[],
    dismiss: (_id: string) => {},
    restore: (_id: string) => {},
  };
  function Probe({ id }: { id: string }) {
    const h = useSessionExplains(id);
    seen.explains = h.explains;
    seen.dismiss = h.dismiss;
    seen.restore = h.restore;
    return null;
  }
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<Probe id={sessionId} />));
  return {
    probe: seen,
    switchTo: async (id: string) => {
      await act(async () => {
        root!.render(<Probe id={id} />);
      });
    },
    settle: async () => {
      await act(async () => {});
    },
  };
}

describe("useSessionExplains dismissal", () => {
  /** The bug this closes: ✕ used to live in this hook's state alone, so
   *  switching away and back re-read the store and handed the card straight
   *  back — "我已经 x 掉了，下一次切换到这个任务还是会弹出来". */
  it("keeps a dismissed card out of the rail after a session switch", async () => {
    const { probe, switchTo, settle } = mount("s1");
    await settle();
    expect(probe.explains.map((r) => r.id)).toEqual(["e1", "e2"]);

    act(() => probe.dismiss("e1"));
    expect(probe.explains.map((r) => r.id)).toEqual(["e2"]);

    await switchTo("s2");
    await switchTo("s1");

    expect(probe.explains.map((r) => r.id)).toEqual(["e2"]);
  });

  it("hands a restored card back, and that survives the switch too", async () => {
    store.dismissed.add("e1");
    const { probe, switchTo, settle } = mount("s1");
    await settle();
    expect(probe.explains.map((r) => r.id)).toEqual(["e2"]);

    act(() => probe.restore("e1"));
    expect(probe.explains.map((r) => r.id)).toEqual(["e1", "e2"]);

    await switchTo("s2");
    await switchTo("s1");

    expect(probe.explains.map((r) => r.id)).toEqual(["e1", "e2"]);
  });

  /** A refused ask never reached the store, so there is nothing to persist —
   *  the card still has to leave the rail on the click. */
  it("hides a local-only error card without calling the host", async () => {
    const { dismissExplanation } = await import("../explainApi");
    store.records = [rec("local-1", "s1")];
    const { probe, settle } = mount("s1");
    await settle();

    act(() => probe.dismiss("local-1"));

    expect(probe.explains).toEqual([]);
    expect(dismissExplanation).not.toHaveBeenCalled();
  });
});
