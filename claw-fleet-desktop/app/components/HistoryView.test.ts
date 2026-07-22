import { describe, expect, it } from "vitest";
import {
  applyFrozenOrder,
  chainBarColor,
  CHAT_HIDDEN,
  CHAT_ONLY,
  dwellReadTargets,
  matchesWorkspaceFilter,
  sessionPaneStyle,
  sessionEq,
} from "./HistoryView";
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
    lastReadMs: null,
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

/**
 * A collapsed relay group's header aggregates its unread dot over the *whole*
 * chain, but clicking the header only opens the tip. `dwellReadTargets` is what
 * makes the 2s dwell-read clear the whole chain for a group header, so a
 * non-tip unread hop can't keep the group lit after the user opened it — the
 * exact bug this covers. Singles (and expanded-group children, which are not in
 * the `groupTips` map) must still mark only themselves.
 */
describe("dwellReadTargets", () => {
  const s = (id: string): SessionInfo => ({ ...base(), id }) as SessionInfo;

  it("marks the whole chain when the active session is a group header (tip)", () => {
    const tip = s("tip");
    const chain = [tip, s("hop2"), s("hop1")];
    const groupTips = new Map([["tip", chain]]);
    // The tip alone is idle-unread-clear; the reason the group is lit is hop2,
    // so the targets must cover every member, not just the tip.
    expect(dwellReadTargets(tip, groupTips).map((m) => m.id)).toEqual([
      "tip",
      "hop2",
      "hop1",
    ]);
  });

  it("marks only itself for a standalone row (id absent from the group map)", () => {
    const solo = s("solo");
    expect(dwellReadTargets(solo, new Map()).map((m) => m.id)).toEqual(["solo"]);
  });

  it("marks only itself for an expanded group's child (a non-tip hop)", () => {
    // Only the tip id keys the map; a child row's dwell must not sweep the chain.
    const groupTips = new Map([["tip", [s("tip"), s("child")]]]);
    expect(dwellReadTargets(s("child"), groupTips).map((m) => m.id)).toEqual(["child"]);
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
 * The workspace `<select>` mixes real paths with two pseudo-values for the
 * pure-chat workspace. The cases that matter: chat is reachable on its own, it
 * can be excluded from the default view, and a chat path the backend never
 * handed us must not silently empty the rail.
 */
describe("matchesWorkspaceFilter", () => {
  const CHAT = "/Users/foo/.fleet/chat";
  const chatSession = { ...base(), workspacePath: CHAT } as SessionInfo;
  const repoSession = { ...base(), workspacePath: "/Users/foo/repo" } as SessionInfo;

  it("passes everything under 'all', chat included", () => {
    expect(matchesWorkspaceFilter(chatSession, "all", CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(repoSession, "all", CHAT)).toBe(true);
  });

  it("keeps only chat sessions under CHAT_ONLY", () => {
    expect(matchesWorkspaceFilter(chatSession, CHAT_ONLY, CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(repoSession, CHAT_ONLY, CHAT)).toBe(false);
  });

  it("drops chat sessions under CHAT_HIDDEN", () => {
    expect(matchesWorkspaceFilter(chatSession, CHAT_HIDDEN, CHAT)).toBe(false);
    expect(matchesWorkspaceFilter(repoSession, CHAT_HIDDEN, CHAT)).toBe(true);
  });

  it("still matches a plain workspace path", () => {
    expect(matchesWorkspaceFilter(repoSession, "/Users/foo/repo", CHAT)).toBe(true);
    expect(matchesWorkspaceFilter(chatSession, "/Users/foo/repo", CHAT)).toBe(false);
  });

  // The dropdown collapses in-repo worktree checkouts onto their repo root
  // (via distinctWorkspaces), so the option value is the root. A session
  // running inside `<repo>/.worktrees/<task>` must therefore match the root
  // filter — an exact-path compare would silently omit it from the view.
  it("matches a worktree-checkout session against its repo-root filter", () => {
    const worktreeSession = {
      ...base(),
      workspacePath: "/Users/foo/repo/.worktrees/fix-bug",
    } as SessionInfo;
    expect(matchesWorkspaceFilter(worktreeSession, "/Users/foo/repo", CHAT)).toBe(true);
  });

  it("degrades both chat pseudo-values to 'all' when the chat path is unknown", () => {
    for (const s of [chatSession, repoSession]) {
      expect(matchesWorkspaceFilter(s, CHAT_ONLY, null)).toBe(true);
      expect(matchesWorkspaceFilter(s, CHAT_HIDDEN, null)).toBe(true);
    }
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
describe("sessionPaneStyle", () => {
  it("keeps a hidden pane in layout without accepting input", () => {
    expect(sessionPaneStyle(false)).toEqual({
      visibility: "hidden",
      position: "absolute",
      inset: 0,
      pointerEvents: "none",
    });
  });

  it("lets the active pane participate in the detail flex row", () => {
    expect(sessionPaneStyle(true)).toEqual({
      visibility: "visible",
      position: "relative",
      pointerEvents: "auto",
    });
  });
});
