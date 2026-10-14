import { describe, expect, it } from "vitest";
import type { SessionInfo, SessionStatus } from "../../types";
import { buildRoster, layoutFleet, POD_LABEL_H, RECENT_MS, SLOT_H, SLOT_W, type PodIn } from "./layout";

const NOW = 10_000_000;

function session(id: string, ws: string, status: SessionStatus, extra: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id,
    workspaceName: ws,
    status,
    isSubagent: false,
    parentSessionId: null,
    procAlive: false,
    lastActivityMs: NOW - 1000,
    createdAtMs: 0,
    ...extra,
  } as SessionInfo;
}

const pod = (key: string, n: number, shrimp = 0): PodIn => ({
  key,
  crew: Array.from({ length: n }, (_, i) => ({
    session: session(`${key}-${i}`, key, "thinking"),
    subagents: Array.from({ length: shrimp }, (_, k) => session(`${key}-${i}-s${k}`, key, "executing")),
  })),
});

describe("buildRoster", () => {
  it("keeps live and recent main sessions, drops long-ended ones", () => {
    const pods = buildRoster(
      [
        session("live", "a", "executing", { lastActivityMs: 0 }),
        session("recent", "a", "idle", { lastActivityMs: NOW - RECENT_MS / 2 }),
        session("old", "a", "idle", { lastActivityMs: NOW - RECENT_MS * 2 }),
        session("alive-quiet", "b", "idle", { lastActivityMs: 0, procAlive: true }),
        session("watch", "c", "watching", { lastActivityMs: 0 }),
      ],
      NOW,
    );
    expect(pods.flatMap((p) => p.crew.map((c) => c.session.id)).sort()).toEqual(
      ["alive-quiet", "live", "recent", "watch"].sort(),
    );
  });

  it("never hides a session the caller wants kept (a card is waiting)", () => {
    const pods = buildRoster([session("carded", "a", "idle", { lastActivityMs: 0 })], NOW, (id) => id === "carded");
    expect(pods).toHaveLength(1);
  });

  it("hangs live sub-agents under their parent and skips finished ones", () => {
    const pods = buildRoster(
      [
        session("cap", "a", "delegating"),
        session("s1", "a", "executing", { isSubagent: true, parentSessionId: "cap" }),
        session("s2", "a", "idle", { isSubagent: true, parentSessionId: "cap" }),
      ],
      NOW,
    );
    expect(pods).toHaveLength(1);
    expect(pods[0].crew).toHaveLength(1);
    expect(pods[0].crew[0].subagents.map((s) => s.id)).toEqual(["s1"]);
  });

  it("puts busy pods first, then by name so pods do not reshuffle on activity", () => {
    const pods = buildRoster(
      [
        session("resting", "a-quiet", "idle", { lastActivityMs: NOW - 10 }),
        session("older-busy", "b-busy", "executing", { lastActivityMs: NOW - 5000 }),
        session("newer-busy", "c-busy", "thinking", { lastActivityMs: NOW - 100 }),
      ],
      NOW,
    );
    expect(pods.map((p) => p.key)).toEqual(["b-busy", "c-busy", "a-quiet"]);
  });

  it("seats a pod's crew by creation time", () => {
    const pods = buildRoster(
      [session("b", "a", "thinking", { createdAtMs: 2 }), session("a", "a", "idle", { createdAtMs: 1 })],
      NOW,
    );
    expect(pods[0].crew.map((c) => c.session.id)).toEqual(["a", "b"]);
  });
});

describe("layoutFleet", () => {
  it("places every seat inside its pod, below the label strip", () => {
    const { pods, hiddenPods } = layoutFleet([pod("a", 3, 1), pod("b", 1)], 400, 200);
    expect(hiddenPods).toBe(0);
    for (const p of pods) {
      for (const s of p.seats) {
        expect(s.x).toBeGreaterThanOrEqual(p.x);
        expect(s.y).toBeGreaterThanOrEqual(p.y + POD_LABEL_H);
        expect(s.x + SLOT_W - 6).toBeLessThanOrEqual(p.x + p.w);
        expect(s.y + SLOT_H).toBeLessThanOrEqual(p.y + p.h);
      }
    }
    expect(pods[0].seats[0].shrimp).toHaveLength(1);
  });

  it("fits twenty-plus sessions on a desktop-sized sea", () => {
    const many = Array.from({ length: 8 }, (_, i) => pod(`ws${i}`, 3));
    const { pods, hiddenPods } = layoutFleet(many, 426, 226);
    expect(hiddenPods).toBe(0);
    expect(pods.reduce((n, p) => n + p.seats.length, 0)).toBe(24);
    // No two pods overlap.
    for (const a of pods)
      for (const b of pods)
        if (a !== b) expect(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y).toBe(true);
  });

  it("wraps a crowded workspace onto more rows instead of overflowing the width", () => {
    const { pods } = layoutFleet([pod("big", 15)], 200, 400);
    expect(pods[0].x + pods[0].w).toBeLessThanOrEqual(200);
    expect(pods[0].seats).toHaveLength(15);
  });

  it("counts pods and crew that do not fit instead of squeezing them", () => {
    const { pods, hiddenPods } = layoutFleet([pod("a", 30), pod("b", 1)], 200, 120);
    expect(pods).toHaveLength(1);
    expect(pods[0].more).toBe(30 - pods[0].seats.length);
    expect(pods[0].more).toBeGreaterThan(0);
    expect(hiddenPods).toBe(1);
  });

  it("caps the drawn shrimp per seat", () => {
    const { pods } = layoutFleet([pod("a", 1, 5)], 400, 200);
    expect(pods[0].seats[0].shrimp).toHaveLength(2);
  });
});
