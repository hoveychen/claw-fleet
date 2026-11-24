import { describe, expect, it } from "vitest";

import type { QueuedRequest, ResourceRequest, ResourceSnapshot } from "./generated/types";
import type { ResourceEvent } from "./generated/types";
import {
  eventDetails,
  eventKind,
  eventTime,
  eventTitle,
  commandLabel,
  fmtDuration,
  moveTargets,
  resourceCards,
  sessionHolds,
  waitLines,
  workspaceName,
} from "./resources";

function req(id: string, needs: Record<string, number>, command = `run ${id}`): ResourceRequest {
  return {
    id,
    kind: "job",
    needs,
    priority: "normal",
    enqueuedMs: 0,
    manual: false,
    holder: { pid: 1 },
    command,
  };
}

function queued(position: number, r: ResourceRequest, waitOn: QueuedRequest["waitOn"] = []): QueuedRequest {
  return { position, request: r, waitOn };
}

// Global queue: a(gpu) b(db) c(gpu+db) d(gpu). Holder h holds gpu.
const h = req("h", { gpu: 1 }, "python train.py --epochs 40");
const a = queued(1, req("a", { gpu: 1 }), [{ resource: "gpu", reason: "busy", by: ["h"] }]);
const b = queued(2, req("b", { db: 1 }));
const c = queued(3, req("c", { gpu: 1, db: 1 }), [
  { resource: "gpu", reason: "busy", by: ["h"] },
  { resource: "db", reason: "reserved", by: ["b"] },
]);
const d = queued(4, req("d", { gpu: 1 }));
const SNAP: ResourceSnapshot = {
  nowMs: 1000,
  resources: [
    { name: "gpu", capacity: 1, inUse: 1, holders: ["h"], waiters: ["a", "c", "d"] },
    { name: "db", capacity: 1, inUse: 0, holders: [], waiters: ["b", "c"] },
  ],
  holders: [h],
  queue: [a, b, c, d],
};

/** Apply a MoveTarget the way `resource_queue.rs::State::move_request` does. */
function applyMove(ids: string[], id: string, target: { index: number } | { before: string }): string[] {
  const rest = ids.filter((x) => x !== id);
  const to = "index" in target ? Math.min(target.index - 1, rest.length) : rest.indexOf(target.before);
  rest.splice(to, 0, id);
  return rest;
}

describe("resourceCards", () => {
  it("resolves holders and waiters per resource in queue order", () => {
    const [gpu, db] = resourceCards(SNAP);
    expect(gpu.holders.map((r) => r.id)).toEqual(["h"]);
    expect(gpu.waiters.map((q) => q.request.id)).toEqual(["a", "c", "d"]);
    expect(db.waiters.map((q) => q.request.id)).toEqual(["b", "c"]);
  });

  it("drops ids the snapshot no longer carries instead of crashing", () => {
    const snap = { ...SNAP, holders: [], queue: [a] };
    const [gpu] = resourceCards(snap);
    expect(gpu.holders).toEqual([]);
    expect(gpu.waiters.map((q) => q.request.id)).toEqual(["a"]);
  });
});

describe("moveTargets", () => {
  const gpuWaiters = resourceCards(SNAP)[0].waiters;
  const order = ["a", "b", "c", "d"];

  it("disables top/up for the global head and down for the card's last", () => {
    expect(moveTargets(gpuWaiters, 0)).toMatchObject({ top: null, up: null });
    expect(moveTargets(gpuWaiters, 2).down).toBeNull();
  });

  it("up lands directly ahead of the previous waiter on the card", () => {
    const t = moveTargets(gpuWaiters, 1).up!; // c, previous gpu waiter is a
    expect(applyMove(order, "c", t)).toEqual(["c", "a", "b", "d"]);
  });

  it("down lands directly behind the next waiter on the card, skipping others", () => {
    const t = moveTargets(gpuWaiters, 0).down!; // a, next gpu waiter is c
    expect(applyMove(order, "a", t)).toEqual(["b", "c", "a", "d"]);
    const t2 = moveTargets(gpuWaiters, 1).down!; // c behind d
    expect(applyMove(order, "c", t2)).toEqual(["a", "b", "d", "c"]);
  });

  it("top is the head of the whole queue", () => {
    const t = moveTargets(gpuWaiters, 2).top!;
    expect(applyMove(order, "d", t)).toEqual(["d", "a", "b", "c"]);
  });

  it("returns all-null for an index out of range", () => {
    expect(moveTargets(gpuWaiters, 9)).toEqual({ top: null, up: null, down: null });
  });
});

describe("labels", () => {
  it("formats durations compactly", () => {
    expect(fmtDuration(-5)).toBe("0s");
    expect(fmtDuration(45_000)).toBe("45s");
    expect(fmtDuration(12 * 60_000)).toBe("12m");
    expect(fmtDuration(3 * 3_600_000 + 5 * 60_000)).toBe("3h 5m");
    expect(fmtDuration(2 * 86_400_000)).toBe("2d");
  });

  it("collapses whitespace, truncates, and marks a private command", () => {
    expect(commandLabel(req("x", {}, "cargo   test\n  -p core"))).toBe("cargo test -p core");
    expect(commandLabel(req("x", {}, "y".repeat(100)), 10)).toBe(`${"y".repeat(9)}…`);
    expect(commandLabel(req("x", {}, ""))).not.toBe("");
  });

  it("takes the last path segment of a workspace", () => {
    expect(workspaceName("/Users/boss/workspace/claude-fleet")).toBe("claude-fleet");
    expect(workspaceName(null)).toBeNull();
  });

  it("names the blocking request by its command, falling back to the id", () => {
    const lines = waitLines(c, SNAP);
    expect(lines).toHaveLength(2);
    expect(lines[0]).toContain("gpu");
    expect(lines[0]).toContain("python train.py");
    expect(lines[1]).toContain("run b");
    const gone = queued(5, req("z", { gpu: 1 }), [{ resource: "gpu", reason: "busy", by: ["vanished"] }]);
    expect(waitLines(gone, SNAP)[0]).toContain("vanished");
  });

  it("reads a session lease by its note, never as an incognito command", () => {
    const lease: ResourceRequest = { ...req("l1", { board: 1 }, ""), kind: "lease", note: "flash   firmware", sessionId: "5d2e8f14-aaaa" };
    expect(commandLabel(lease)).toContain("flash firmware");
    expect(commandLabel(lease)).not.toBe(commandLabel(req("x", {}, "")));
    const bare = commandLabel({ ...lease, note: null });
    expect(bare).not.toBe("");
    expect(bare).not.toBe(commandLabel(req("x", {}, "")));
  });

  it("says a not-yet-due lease waits for its time once, not once per resource", () => {
    const now = new Date(2026, 10, 23, 15, 0, 0).getTime();
    const later = new Date(2026, 10, 23, 15, 40, 0).getTime();
    const lease: ResourceRequest = { ...req("l2", { board: 1, db: 1 }, ""), kind: "lease", afterMs: later };
    const q = queued(1, lease, [
      { resource: "board", reason: "after", by: [] },
      { resource: "db", reason: "after", by: [] },
    ]);
    const lines = waitLines(q, { ...SNAP, nowMs: now });
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("15:40");
  });
});

describe("history rows", () => {
  const released = {
    tsMs: 0,
    kind: "released",
    requestId: "r1",
    needs: { gpu: 2, db: 1 },
    command: "python   train.py\n --epochs 3",
    heldMs: 125_000,
    exitCode: 1,
  } as ResourceEvent;

  it("titles a request event by its one-line command and a table event by its resource", () => {
    expect(eventTitle(released)).toBe("python train.py --epochs 3");
    expect(eventTitle({ tsMs: 0, kind: "capacity_set", resource: "gpu", capacity: 3, prevCapacity: 1 } as ResourceEvent)).toContain("1 → 3");
    expect(eventTitle({ tsMs: 0, kind: "granted", requestId: "r9" } as ResourceEvent)).toBe("r9");
  });

  it("lists needs, hold time and exit code", () => {
    const d = eventDetails(released);
    expect(d).toContain("gpu:2, db");
    expect(d).toContain("2m");
    expect(d).toContain("1");
  });

  it("gives every kind a label and a tone", () => {
    for (const k of ["enqueued", "granted", "released", "kicked", "cancelled", "timed_out", "aborted", "reaped", "transferred", "capacity_set", "resource_removed"] as const) {
      expect(eventKind(k).label).not.toBe("");
      expect(eventKind(k).label).not.toBe(k);
    }
    expect(eventKind("kicked").tone).toBe("bad");
  });

  it("titles a lease event by its note and shows which sessions it moved between", () => {
    const moved = {
      tsMs: 0,
      kind: "transferred",
      requestId: "l1",
      requestKind: "lease",
      needs: { board: 1 },
      note: "flash firmware",
      sessionId: "1f8a6c3d-92e4",
      fromSessionId: "7c41d9e2-b03a",
    } as ResourceEvent;
    expect(eventTitle(moved)).toContain("flash firmware");
    const d = eventDetails(moved);
    expect(d).toContain("7c41d9e2");
    expect(d).toContain("1f8a6c3d");
    expect(eventTitle({ ...moved, note: null })).not.toBe("l1");
  });

  it("shows only the time for today and the date otherwise", () => {
    const now = new Date(2026, 10, 23, 15, 0, 0).getTime();
    expect(eventTime(new Date(2026, 10, 23, 9, 5, 7).getTime(), now)).toBe("09:05:07");
    expect(eventTime(new Date(2026, 10, 20, 9, 5, 7).getTime(), now)).toBe("11-20 09:05");
  });
});

describe("lease pieces", () => {
  const lease = (id: string, sessionId: string, needs: Record<string, number>): ResourceRequest => ({
    ...req(id, needs, ""),
    kind: "lease",
    sessionId,
  });
  it("sessionHolds lists what the queued piece's session already holds", () => {
    const holders = [lease("a", "s1", { round: 1 }), lease("b", "s2", { desk: 1 }), req("j", { gpu: 1 })];
    expect(sessionHolds(lease("q", "s1", { wave: 1 }), holders)).toBe("round");
    expect(sessionHolds(lease("q", "s3", { wave: 1 }), holders)).toBe("");
    expect(sessionHolds(req("j2", { round: 1 }), holders)).toBe("");
  });
  it("eventDetails shows what a partial release kept", () => {
    const e: ResourceEvent = { tsMs: 0, kind: "released", requestKind: "lease", needs: { round: 1 }, kept: { wave: 1 } };
    expect(eventDetails(e)).toContain("wave");
  });
});
