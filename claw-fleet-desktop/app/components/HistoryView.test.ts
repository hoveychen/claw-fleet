import { describe, expect, it } from "vitest";
import {
  applyFrozenOrder,
  taskListSessions,
} from "./HistoryView";
import { NEW_SESSION_ENTRYPOINT } from "../types";
import { chainBarColor } from "./sessionGroups";
import { sessionEq } from "./SessionRow";
import type { SessionInfo, SessionStatus } from "../types";

/**
 * `sessionEq` backs SessionRow's memo. Two failure modes matter and they pull
 * in opposite directions:
 *
 *   too strict → memo never hits, the launchpad re-renders every row every
 *                scan tick (the bug the memo exists to fix)
 *   too loose  → a changed row silently freezes on screen (a correctness bug,
 *                strictly worse than the perf one)
 *
 * The second is why this compares by the object's own keys instead of a
 * hand-written field list, and why the cases below walk the fields the row and
 * its child controls (MarkControl, StopControl) actually read.
 */

function base(): SessionInfo {
  return {
    id: "s1",
    jsonlPath: "/p/s1.jsonl",
    workspacePath: "/w",
    workspaceName: "w",
    status: "idle",
    aiTitle: "title",
    slug: null,
    lastMessagePreview: "hi",
    lastActivityMs: 1000,
    createdAtMs: 500,
    pid: 42,
    pidPrecise: true,
    isSubagent: false,
    entrypoint: null,
    agentSource: "claude-code",
    userMark: null,
    handoff: null,
    taskPlan: null,
  } as unknown as SessionInfo;
}

function member(status: SessionStatus, hop: number): SessionInfo {
  return {
    ...base(),
    status,
    handoff: { hop, chainLen: 5, chainId: "c" },
  } as unknown as SessionInfo;
}

const SUCCESS = "var(--color-success)";
const WARNING = "var(--color-warning)";

describe("chainBarColor (collapsed relay-group header liveness)", () => {
  it("is null when no member is live", () => {
    expect(chainBarColor([member("idle", 1), member("idle", 2)])).toBe(null);
  });

  it("surfaces a live mid-chain hop even when the tip is done", () => {
    // The bug: the tip (highest hop) is idle, but hop 2 is executing. The header
    // must show the chain as live rather than inheriting only the tip's null.
    const members = [member("executing", 2), member("idle", 3)];
    expect(chainBarColor(members)).toBe(SUCCESS);
  });

  it("shows amber when the only live member is waiting for input", () => {
    expect(chainBarColor([member("waitingInput", 2), member("idle", 3)])).toBe(WARNING);
  });

  it("prefers a running member (green) over a waiting one (amber)", () => {
    expect(chainBarColor([member("waitingInput", 1), member("executing", 2)])).toBe(SUCCESS);
  });
});

describe("sessionEq", () => {
  it("treats a fresh deserialisation of identical data as equal", () => {
    // The scanner hands the UI a brand-new object every ~2s. Reference equality
    // is always false there, so this is the case the memo depends on.
    const a = base();
    const b = base();
    expect(a).not.toBe(b);
    expect(sessionEq(a, b)).toBe(true);
  });

  it("is reflexive on the same reference", () => {
    const a = base();
    expect(sessionEq(a, a)).toBe(true);
  });

  // Fields the row itself renders.
  it.each([
    ["aiTitle", "changed"],
    ["lastMessagePreview", "new msg"],
    ["lastActivityMs", 2000],
    ["createdAtMs", 999],
    ["workspaceName", "other"],
    ["status", "thinking"],
  ] as const)("detects a change to %s", (key, value) => {
    const a = base();
    const b = { ...base(), [key]: value } as SessionInfo;
    expect(sessionEq(a, b)).toBe(false);
  });

  // Fields only the child controls read — the ones a hand-maintained field
  // list would most plausibly forget, freezing the mark/stop button.
  it("detects a userMark flip (MarkControl)", () => {
    const b = { ...base(), userMark: "done" } as SessionInfo;
    expect(sessionEq(base(), b)).toBe(false);
  });

  it("detects the process dying (StopControl)", () => {
    const b = { ...base(), pid: null } as SessionInfo;
    expect(sessionEq(base(), b)).toBe(false);
  });

  it("detects a pidPrecise flip (StopControl)", () => {
    const b = { ...base(), pidPrecise: false } as SessionInfo;
    expect(sessionEq(base(), b)).toBe(false);
  });

  // Nested objects are fresh on every deserialisation, so `===` on them is
  // always false — they must go through the structural compare, in both
  // directions.
  it("treats structurally identical nested handoff as equal", () => {
    const a = { ...base(), handoff: { hop: 1, chainLen: 3 } } as unknown as SessionInfo;
    const b = { ...base(), handoff: { hop: 1, chainLen: 3 } } as unknown as SessionInfo;
    expect(sessionEq(a, b)).toBe(true);
  });

  it("detects a change inside nested handoff", () => {
    const a = { ...base(), handoff: { hop: 1, chainLen: 3 } } as unknown as SessionInfo;
    const b = { ...base(), handoff: { hop: 2, chainLen: 3 } } as unknown as SessionInfo;
    expect(sessionEq(a, b)).toBe(false);
  });

  it("detects null → object on a nested field", () => {
    const b = { ...base(), handoff: { hop: 1, chainLen: 2 } } as unknown as SessionInfo;
    expect(sessionEq(base(), b)).toBe(false);
  });

  it("detects a key appearing that the other side lacks", () => {
    const a = base();
    const b = { ...base(), someNewField: 1 } as unknown as SessionInfo;
    expect(sessionEq(a, b)).toBe(false);
  });
});

/**
 * `taskListSessions` is the single gate for the 任务 page. The regression it
 * pins: simplified mode used to skip the filter and hand the raw scan to the
 * list, so the subagent transcripts `scan.rs` collects
 * (`<parent>/subagents/agent-*.jsonl`) showed up as top-level tasks — a
 * fan-out turn on fleet-cloud produced a dozen rows titled with each
 * subagent's closing sentence. The gate takes no mode argument on purpose.
 */
describe("taskListSessions", () => {
  const s = (over: Partial<SessionInfo>): SessionInfo =>
    ({
      id: "x",
      entrypoint: NEW_SESSION_ENTRYPOINT,
      isSubagent: false,
      fleetSpawned: true,
      ...over,
    }) as SessionInfo;

  it("drops subagent transcripts the scan collects", () => {
    const rows = [
      s({ id: "parent" }),
      s({ id: "agent-a1", isSubagent: true }),
      s({ id: "agent-a2", isSubagent: true }),
    ];
    expect(taskListSessions(rows).map((r) => r.id)).toEqual(["parent"]);
  });

  it("drops sessions Fleet did not spawn", () => {
    const rows = [s({ id: "own" }), s({ id: "outside", entrypoint: "cli", fleetSpawned: false })];
    expect(taskListSessions(rows).map((r) => r.id)).toEqual(["own"]);
  });
});

/**
 * `applyFrozenOrder` holds the list still while the pointer is parked over it.
 * The behaviours that matter: it preserves the frozen order (not the live sort),
 * appends genuinely new rows at the end (never splices them into the frozen
 * block — that reflow under the cursor is the whole bug this fixes), drops rows
 * that vanished, and keeps each row's *live* content.
 */
describe("applyFrozenOrder", () => {
  const row = (id: string, activity: number): SessionInfo =>
    ({ ...base(), id, jsonlPath: `/p/${id}.jsonl`, agentLastActivityMs: activity }) as SessionInfo;

  it("passes rows through untouched when nothing is frozen", () => {
    const rows = [row("a", 3), row("b", 2)];
    expect(applyFrozenOrder(rows, null)).toBe(rows);
  });

  it("keeps the frozen order even after the live list re-sorts", () => {
    // Frozen while a<b<c; then a scan makes c the most recent, flipping the live
    // sort. The frozen order must win.
    const live = [row("c", 9), row("a", 3), row("b", 2)];
    const out = applyFrozenOrder(live, ["a", "b", "c"]);
    expect(out.map((s) => s.id)).toEqual(["a", "b", "c"]);
  });

  it("appends a newly-arrived session at the end, not into the frozen block", () => {
    // `n` is the freshest by activity but was absent when the order froze; it
    // must land last so no frozen row shifts under the cursor.
    const live = [row("n", 99), row("a", 3), row("b", 2)];
    const out = applyFrozenOrder(live, ["a", "b"]);
    expect(out.map((s) => s.id)).toEqual(["a", "b", "n"]);
  });

  it("drops a frozen id whose row has left the live list", () => {
    const live = [row("a", 3)];
    const out = applyFrozenOrder(live, ["a", "gone"]);
    expect(out.map((s) => s.id)).toEqual(["a"]);
  });

  it("resolves each id against the live row so content stays fresh", () => {
    // The returned object for `a` must be the live one (activity 42), not a
    // stale snapshot — only the *order* is frozen.
    const live = [row("a", 42)];
    const out = applyFrozenOrder(live, ["a"]);
    expect(out[0]).toBe(live[0]);
  });
});

/**
 * WebKit can leave a restored overflow scroller visually blank when an
 * ancestor crosses `display: none` -> `flex`; the next physical scroll then
 * repaints it. Hidden session tabs must therefore stay laid out while being
 * invisible, so their scroll layer and ResizeObserver never collapse to zero.
 */
