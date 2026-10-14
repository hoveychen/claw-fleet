import { describe, expect, it } from "vitest";
import type { SessionInfo, SessionStatus } from "../../types";
import {
  buildRoster,
  layoutBridge,
  layoutShips,
  RECENT_MS,
  SHIP_CELL_H,
  SLOT_H,
  SLOT_W,
  type PodIn,
} from "./layout";
import { SHIP_H, SHIP_W } from "./ship";
import { LOBSTER_W } from "./sprites";

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

const overlap = (a: { x: number; y: number }, b: { x: number; y: number }, w: number, h: number) =>
  !(a.x + w <= b.x || b.x + w <= a.x || a.y + h <= b.y || b.y + h <= a.y);

describe("layoutShips", () => {
  it("places every workspace's ship inside the sky without overlaps", () => {
    const pods = Array.from({ length: 15 }, (_, i) => pod(`ws${i}`, 1));
    const { ships, hidden } = layoutShips(pods, 426, 240);
    expect(hidden).toBe(0);
    expect(ships).toHaveLength(15);
    for (const a of ships) {
      expect(a.x).toBeGreaterThanOrEqual(0);
      expect(a.x + SHIP_W).toBeLessThanOrEqual(426);
      expect(a.y + SHIP_H).toBeLessThanOrEqual(240);
      for (const b of ships) if (a !== b) expect(overlap(a, b, SHIP_W, SHIP_H)).toBe(false);
    }
  });

  it("centres a lone ship", () => {
    const { ships } = layoutShips([pod("solo", 2)], 400, 200);
    expect(Math.abs(ships[0].x + SHIP_W / 2 - 200)).toBeLessThanOrEqual(1);
    expect(Math.abs(ships[0].y - 100)).toBeLessThan(SHIP_CELL_H);
  });

  it("counts the workspaces that do not fit", () => {
    const pods = Array.from({ length: 40 }, (_, i) => pod(`ws${i}`, 1));
    const { ships, hidden } = layoutShips(pods, 200, 100);
    expect(ships.length + hidden).toBe(40);
    expect(hidden).toBeGreaterThan(0);
  });
});

describe("layoutBridge", () => {
  it("seats the crew below the viewport, inside the bridge", () => {
    const crew = pod("a", 7, 1).crew;
    const b = layoutBridge(crew, 426, 240);
    expect(b.more).toBe(0);
    expect(b.seats).toHaveLength(7);
    for (const s of b.seats) {
      expect(s.y).toBeGreaterThan(b.viewportH);
      expect(s.x + LOBSTER_W).toBeLessThanOrEqual(426);
      expect(s.y + SLOT_H).toBeLessThanOrEqual(240);
      expect(s.shrimp).toHaveLength(1);
    }
    for (const a of b.seats) for (const c of b.seats) if (a !== c) expect(overlap(a, c, SLOT_W - 6, SLOT_H)).toBe(false);
    expect(b.rowTops.length).toBeGreaterThan(0);
  });

  it("counts crew that do not fit instead of squeezing them", () => {
    const b = layoutBridge(pod("a", 60).crew, 200, 160);
    expect(b.more).toBe(60 - b.seats.length);
    expect(b.more).toBeGreaterThan(0);
  });

  it("caps the drawn shrimp per seat", () => {
    expect(layoutBridge(pod("a", 1, 5).crew, 400, 200).seats[0].shrimp).toHaveLength(2);
  });
});
