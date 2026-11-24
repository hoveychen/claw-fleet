import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";
import type { QueuedRequest, ResourceRequest } from "../generated/types";
import { fmtClock, moveTargetFor, requestLabel, sessionHolds } from "./ResourcesView";

/** Returns the inline default, as i18next does for a missing key. */
const t = ((key: string, def?: string) => def ?? key) as unknown as TFunction;

describe("requestLabel", () => {
  const base = { id: "x", needs: {}, priority: "normal", enqueuedMs: 0, manual: false, holder: { pid: 0 } } as const;
  it("names a command by its one-line command", () => {
    expect(requestLabel({ ...base, kind: "job", command: "cargo   test\n -p core" } as ResourceRequest, t)).toBe("cargo test -p core");
  });
  it("names a session lease by its note, not as an incognito command", () => {
    const lease = { ...base, kind: "lease", command: "", note: "flash  firmware" } as ResourceRequest;
    expect(requestLabel(lease, t)).toBe("flash firmware");
    const bare = requestLabel({ ...lease, note: null }, t);
    expect(bare).not.toBe(requestLabel({ ...base, kind: "job", command: "" } as ResourceRequest, t));
    expect(requestLabel({ ...lease, note: "y".repeat(100) }, t, 10)).toBe(`${"y".repeat(9)}…`);
  });
});

describe("fmtClock", () => {
  it("shows only the time today and the date on another day", () => {
    const now = new Date(2026, 10, 23, 15, 0).getTime();
    expect(fmtClock(new Date(2026, 10, 23, 15, 40).getTime(), now)).toBe("15:40");
    expect(fmtClock(new Date(2026, 10, 24, 1, 5).getTime(), now)).toBe("11-24 01:05");
  });
});

const queue = ["a", "b", "c", "d"].map(
  (id, i) => ({ position: i + 1, request: { id }, waitOn: [] }) as unknown as QueuedRequest,
);

describe("moveTargetFor", () => {
  it("drops before a row as before:<row>", () => {
    expect(moveTargetFor(queue, "d", "b", false)).toEqual({ before: "b" });
  });
  it("drops after a row as before:<the row that follows it>", () => {
    expect(moveTargetFor(queue, "a", "c", true)).toEqual({ before: "d" });
  });
  it("drops after the last row as the last index", () => {
    expect(moveTargetFor(queue, "a", "d", true)).toEqual({ index: 4 });
  });
  it("ignores drops that change nothing", () => {
    expect(moveTargetFor(queue, "b", "b", true)).toBeNull();
    expect(moveTargetFor(queue, "b", "c", false)).toBeNull();
    expect(moveTargetFor(queue, "c", "b", true)).toBeNull();
    expect(moveTargetFor(queue, "zz", "b", true)).toBeNull();
  });
});

describe("sessionHolds", () => {
  const lease = (id: string, sessionId: string, needs: Record<string, number>, grantedMs: number | null) =>
    ({ id, kind: "lease", sessionId, needs, priority: "normal", enqueuedMs: 0, grantedMs, manual: false, holder: { pid: 0 }, command: "" }) as ResourceRequest;
  it("sums the granted pieces of the queued piece's session", () => {
    const pending = lease("q", "s1", { wave: 1 }, null);
    const byId = new Map(
      [lease("a", "s1", { round: 1 }, 1), lease("b", "s1", { gpu: 2 }, 1), lease("c", "s2", { desk: 1 }, 1), pending].map((r) => [r.id, r]),
    );
    expect(sessionHolds(pending, byId)).toBe("round, gpu:2");
    expect(sessionHolds(lease("x", "s3", { wave: 1 }, null), byId)).toBe("");
  });
});
