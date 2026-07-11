import { describe, expect, it } from "vitest";
import { sessionEq } from "./HistoryView";
import type { SessionInfo } from "../types";

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
    lastReadMs: null,
    handoff: null,
    taskPlan: null,
  } as unknown as SessionInfo;
}

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
