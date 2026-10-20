// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HandoffChain, RawMessage, SessionInfo } from "./types";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

const { usePrevLegs } = await import("./usePrevLegs");
const { useSessionsStore, INITIAL_TAIL } = await import("./store");

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** Each leg's transcript file was created at hop × 1e6 ms; its first record
 *  is stamped 500 ms later, so the two start-time sources are told apart. */
const createdAt = (hop: number) => hop * 1_000_000;
const HOP_OF: Record<string, number> = { "/t/s1.jsonl": 1, "/t/s2.jsonl": 2, c: 3 };

const sess = (id: string, hop: number, title: string) =>
  ({
    id,
    jsonlPath: `/t/${id}.jsonl`,
    aiTitle: title,
    createdAtMs: createdAt(hop),
    handoff: { chainId: "c", hop, chainLen: 3 },
  }) as unknown as SessionInfo;

const CHAIN: HandoffChain = {
  chainId: "c",
  workspacePath: "/w",
  links: [
    { fromSessionId: "s1", toSessionId: "s2", note: "", handedAt: 1000 },
    { fromSessionId: "s2", toSessionId: "s3", note: "", handedAt: 2000 },
  ],
} as unknown as HandoffChain;

const rows = (prefix: string, n: number): RawMessage[] =>
  Array.from(
    { length: n },
    (_, i) =>
      ({
        type: "user",
        uuid: `${prefix}${i}`,
        timestamp: new Date(createdAt(HOP_OF[prefix]) + 500 + i * 1000).toISOString(),
      }) as unknown as RawMessage,
  );

/** Per-path transcript lengths the fake `get_messages_tail` serves from. */
let lengths: Record<string, number>;

let root: Root;
let container: HTMLDivElement;
let result: ReturnType<typeof usePrevLegs>;

function Probe({ fullyLoaded }: { fullyLoaded: boolean }) {
  result = usePrevLegs({
    session: sess("s3", 3, "third"),
    messages: rows("c", 2),
    fullyLoaded,
    loadEarlier: async () => {},
    isLoadingEarlier: false,
  });
  return null;
}

async function flush() {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
}

beforeEach(() => {
  lengths = { "/t/s2.jsonl": INITIAL_TAIL + 5, "/t/s1.jsonl": 3 };
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string, args: { jsonlPath: string; tail: number }) => {
    if (cmd === "get_handoff_chain") return CHAIN;
    if (cmd === "get_messages_tail") {
      const total = lengths[args.jsonlPath];
      const n = Math.min(total, args.tail);
      return rows(args.jsonlPath, total).slice(total - n);
    }
    throw new Error(`unexpected ${cmd}`);
  });
  useSessionsStore.setState({ sessions: [sess("s1", 1, "first"), sess("s2", 2, "second"), sess("s3", 3, "third")] });
  container = document.createElement("div");
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
});

describe("usePrevLegs", () => {
  it("offers nothing until the current leg is fully loaded", async () => {
    act(() => root.render(<Probe fullyLoaded={false} />));
    await flush();
    expect(result.offer).toBeNull();
    expect(invokeMock).not.toHaveBeenCalled();
    // Nothing prepended: no heading, the current leg's list as is.
    expect(result.messages.some((m) => m._fleetLegHeading)).toBe(false);
    expect(result.readOnlyPrefix).toBe(0);
  });

  it("walks back leg by leg, paging each prepended leg before offering the next", async () => {
    act(() => root.render(<Probe fullyLoaded />));
    await flush();
    expect(result.offer?.hop).toBe(2);

    await act(async () => {
      await result.offer!.onLoad();
    });
    // Leg 2 is longer than one tail: it pages first, no leg-1 offer yet.
    // heading + leg-2 tail + current heading + 2 (current)
    expect(result.messages.length).toBe(1 + INITIAL_TAIL + 1 + 2);
    expect(result.fullyLoaded).toBe(false);
    expect(result.offer).toBeNull();
    // Leg 2's first message is not loaded yet: its start is the file's creation.
    expect(result.messages[0]._fleetLegHeading).toEqual({
      hop: 2,
      sessionId: "s2",
      title: "second",
      startedAt: createdAt(2),
      current: false,
    });
    // The current leg is loaded from its start: its first message's time.
    expect(result.messages[1 + INITIAL_TAIL]._fleetLegHeading).toEqual({
      hop: 3,
      sessionId: "s3",
      title: "third",
      startedAt: createdAt(3) + 500,
      current: true,
    });
    expect(result.messages[2 + INITIAL_TAIL].uuid).toBe("c0");

    await act(async () => {
      await result.loadEarlier();
    });
    expect(result.fullyLoaded).toBe(true);
    expect(result.offer?.hop).toBe(1);
    expect(result.messages[0]._fleetLegHeading?.startedAt).toBe(createdAt(2) + 500);

    await act(async () => {
      await result.offer!.onLoad();
    });
    // heading + 3 (leg 1) + heading + 155 (leg 2) + current heading + 2 (current)
    expect(result.messages.length).toBe(1 + 3 + 1 + INITIAL_TAIL + 5 + 1 + 2);
    expect(result.readOnlyPrefix).toBe(1 + 3 + 1 + INITIAL_TAIL + 5 + 1);
    const headings = result.messages.flatMap((m) => (m._fleetLegHeading ? [m._fleetLegHeading] : []));
    expect(headings.map((h) => [h.hop, h.current])).toEqual([
      [1, false],
      [2, false],
      [3, true],
    ]);
    expect(result.messages[0]._fleetLegHeading?.startedAt).toBe(createdAt(1) + 500);
    // Leg 1 has no predecessor: the walk ends there.
    expect(result.offer).toBeNull();
  });

  it("offers nothing when the predecessor is not in the session scan", async () => {
    useSessionsStore.setState({ sessions: [sess("s3", 3, "third")] });
    act(() => root.render(<Probe fullyLoaded />));
    await flush();
    expect(result.offer).toBeNull();
  });
});
