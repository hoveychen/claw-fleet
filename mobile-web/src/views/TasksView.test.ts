import { describe, expect, it } from "vitest";
import { applyFrozenOrder, statusTone, subtaskStateLabel } from "./TasksView";
import type { SessionInfo } from "../types";

describe("statusTone quiet-alive", () => {
  const row = (over: Partial<SessionInfo>) =>
    ({ id: "s", workspacePath: "/w", workspaceName: "n", status: "idle", ...over }) as SessionInfo;

  it("keeps a dot on a live process whose transcript aged out to idle", () => {
    // The desktop's `isQuietAlive`, mirrored: status is derived from transcript
    // age alone, so a session sitting on one long tool call reads `idle` while
    // its process runs on. Dropping the dot would contradict the composer,
    // which offers to queue a follow-up for that very session.
    expect(statusTone(row({ status: "idle", procAlive: true }))).toBe("quiet");
  });

  it("still shows nothing once the process is gone", () => {
    expect(statusTone(row({ status: "idle", procAlive: false }))).toBe(null);
    expect(statusTone(row({ status: "idle" }))).toBe(null);
  });

  it("does not dim a genuinely working row down to quiet", () => {
    // Distinct ids: the tone mapping is what's under test, and the anti-flicker
    // latch is per session — one id would carry the quiet state across.
    expect(statusTone(row({ id: "w", status: "executing", procAlive: true }))).toBe("working");
    expect(statusTone(row({ id: "i", status: "waitingInput", procAlive: true }))).toBe("waiting");
  });

  it("gives a watch-parked row its own tone instead of no dot", () => {
    // Its process is gone, so the old logic fell all the way through to
    // `return null` — no dot, indistinguishable from an ended row, while a Fleet
    // timer was still going to resume it once the condition fires.
    expect(statusTone(row({ id: "wt", status: "watching", procAlive: false }))).toBe("watching");
  });

  it("does not flick back to working on a single sparse write", () => {
    // Same flicker the desktop row had: a session parked on one long tool call
    // writes a line every few minutes, each write pushes the status back to a
    // live one for its hard window, and the dot alternated quiet → working →
    // quiet. One lone write must not win the bright tone back.
    const now = Date.now();
    const id = "flicker-1";
    expect(
      statusTone(row({ id, status: "idle", procAlive: true, lastActivityMs: now - 200_000 })),
    ).toBe("quiet");
    expect(
      statusTone(row({ id, status: "executing", procAlive: true, lastActivityMs: now - 1_000 })),
    ).toBe("quiet");
  });

  it("goes back to working once two writes land close together", () => {
    const now = Date.now();
    const id = "recover-1";
    statusTone(row({ id, status: "idle", procAlive: true, lastActivityMs: now - 200_000 }));
    statusTone(row({ id, status: "executing", procAlive: true, lastActivityMs: now - 20_000 }));
    expect(
      statusTone(row({ id, status: "executing", procAlive: true, lastActivityMs: now - 1_000 })),
    ).toBe("working");
  });
});

describe("applyFrozenOrder", () => {
  const row = (deviceId: string, id: string) =>
    ({ id, deviceId, workspacePath: "/w", workspaceName: "n", status: "idle" }) as unknown as
      SessionInfo & { deviceId: string };
  const keyOf = (r: { deviceId?: string; id: string }) => `${r.deviceId ?? ""}::${r.id}`;
  const keys = (rows: Array<{ deviceId?: string; id: string }>) =>
    rows.map((r) => `${r.deviceId ?? ""}::${r.id}`);

  it("passes rows through untouched when nothing is frozen", () => {
    const rows = [row("d", "a"), row("d", "b")];
    expect(keys(applyFrozenOrder(rows, null, keyOf))).toEqual(["d::a", "d::b"]);
  });

  it("holds the frozen order even after the fresh sort flipped the rows", () => {
    // Freeze captured a, b, c on screen; new snapshot pushed c to the front.
    const frozen = ["d::a", "d::b", "d::c"];
    const resorted = [row("d", "c"), row("d", "a"), row("d", "b")];
    expect(keys(applyFrozenOrder(resorted, frozen, keyOf))).toEqual(["d::a", "d::b", "d::c"]);
  });

  it("appends sessions born after the freeze at the bottom, never in the middle", () => {
    // New session should naturally sort first; inserting it would push every card down one slot.
    const frozen = ["d::a", "d::b"];
    const rows = [row("d", "new"), row("d", "a"), row("d", "b")];
    expect(keys(applyFrozenOrder(rows, frozen, keyOf))).toEqual(["d::a", "d::b", "d::new"]);
  });

  it("keeps several post-freeze arrivals in their own natural order", () => {
    const frozen = ["d::a"];
    const rows = [row("d", "n2"), row("d", "n1"), row("d", "a")];
    expect(keys(applyFrozenOrder(rows, frozen, keyOf))).toEqual(["d::a", "d::n2", "d::n1"]);
  });

  it("silently drops frozen keys whose session is gone (filtered out or ended)", () => {
    const frozen = ["d::a", "d::gone", "d::b"];
    expect(keys(applyFrozenOrder([row("d", "b"), row("d", "a")], frozen, keyOf))).toEqual([
      "d::a",
      "d::b",
    ]);
  });

  it("scopes the frozen key by device so two machines' same id never swap places", () => {
    const frozen = ["dev-b::s", "dev-a::s"];
    const rows = [row("dev-a", "s"), row("dev-b", "s")];
    const out = applyFrozenOrder(rows, frozen, keyOf);
    expect(keys(out)).toEqual(["dev-b::s", "dev-a::s"]);
  });
});

describe("subtaskStateLabel", () => {
  // The test runner resolves to English (no zh browser locale).
  it("says reported back only for a clean end", () => {
    expect(subtaskStateLabel("review", "clean")).toBe("Reported back");
    // codex / dsh rows carry no reading: keep the old wording.
    expect(subtaskStateLabel("review", undefined)).toBe("Reported back");
    expect(subtaskStateLabel("review", "midTurn")).toBe("Stopped mid-turn");
    expect(subtaskStateLabel("review", "interrupted")).toBe("Interrupted by hand");
    expect(subtaskStateLabel("review", "authFailed")).toBe("Auth failed");
    expect(subtaskStateLabel("review", "failed")).toBe("Permanent error");
  });

  it("leaves non-stopped states to the task state label", () => {
    // A running row's tail is mid-turn by definition; that is not a failure.
    expect(subtaskStateLabel("running", "midTurn")).toBe("In progress");
    expect(subtaskStateLabel("done", "midTurn")).toBe("Done");
  });
});
