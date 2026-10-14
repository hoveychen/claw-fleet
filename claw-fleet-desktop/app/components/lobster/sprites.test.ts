import { describe, expect, it } from "vitest";
import {
  composeLobster,
  composeShrimp,
  frameAt,
  LOBSTER_H,
  LOBSTER_W,
  POSE_FRAMES,
  SHRIMP_H,
  SHRIMP_W,
  SPRITE_DETAIL,
  type Pose,
} from "./sprites";

const POSES = Object.keys(POSE_FRAMES) as Pose[];
const lobster = (pose: Pose, frame = 0, captain = false) =>
  composeLobster({ pose, frame, color: "#e0c24a", captain });
/** Topmost row index containing `ch`, or -1. */
const topRow = (rows: string[], ch: string) => rows.findIndex((r) => r.includes(ch));

describe("composeLobster", () => {
  it("composes every pose and frame at the fixed sprite size", () => {
    for (const pose of POSES) {
      for (let f = 0; f < POSE_FRAMES[pose].n; f++) {
        const rows = lobster(pose, f);
        expect(rows).toHaveLength(LOBSTER_H * SPRITE_DETAIL);
        for (const r of rows) expect(r, pose).toHaveLength(LOBSTER_W * SPRITE_DETAIL);
        // The screen glow (state colour) is always visible.
        expect(rows.join("")).toContain("S");
      }
    }
  });

  it("puts a cap only on captains", () => {
    expect(lobster("idle", 0, true).join("")).toContain("H");
    expect(lobster("idle", 0, false).join("")).not.toContain("H");
  });

  it("centres the eyes and mouth on the screen", () => {
    const rows = lobster("idle", 0, true);
    for (const r of rows) {
      const eyes = r.replace(/[^SZ]/g, ".");
      expect(eyes).toEqual([...eyes].reverse().join(""));
    }
  });

  it("draws a screen face whose eyes change with the pose", () => {
    const face = (pose: Pose) => lobster(pose).filter((r) => r.includes("k")).join("\n");
    expect(face("idle")).toContain("SSS");
    expect(face("think")).not.toEqual(face("idle"));
    expect(face("error")).not.toEqual(face("idle"));
    expect(face("read")).not.toContain("Z");
  });

  it("dresses the mascot in a uniform with gold trim", () => {
    const rows = lobster("idle").join("");
    expect(rows).toContain("u");
    expect(rows).toContain("G");
  });

  it("raises the claws above the eyes when waving", () => {
    const eyes = topRow(lobster("wave"), "S");
    expect(topRow(lobster("wave"), "c")).toBeLessThan(eyes - 3);
    expect(topRow(lobster("idle"), "c")).toBeGreaterThan(eyes);
  });

  it("draws the pose props", () => {
    expect(lobster("type").join("")).toContain("K");
    expect(lobster("read").join("")).toContain("P");
    expect(lobster("coffee").join("")).toContain("M");
    expect(lobster("idle").join("")).not.toMatch(/[KPM]/);
  });

  it("leaves no sleeve stub hanging below a lowered claw", () => {
    for (const pose of ["idle", "stuck", "type"] as Pose[]) {
      // Only the left claw's columns: the body and legs sit lower in the middle.
      const rows = lobster(pose).map((r) => r.slice(0, 16));
      const lowestClaw = rows.reduce((acc, r, i) => (r.includes("c") ? i : acc), -1);
      const below = rows.slice(lowestClaw + 1).join("");
      expect(below, pose).not.toContain("u");
    }
  });

  it("wraps the frame index", () => {
    expect(lobster("type", 2)).toEqual(lobster("type", 0));
  });
});

describe("composeShrimp", () => {
  it("has two frames at the fixed size with a glowing screen and no cap", () => {
    for (const frame of [0, 1]) {
      const rows = composeShrimp({ frame, color: "#4ac26b" });
      expect(rows).toHaveLength(SHRIMP_H * SPRITE_DETAIL);
      for (const r of rows) expect(r).toHaveLength(SHRIMP_W * SPRITE_DETAIL);
      expect(rows.join("")).toContain("S");
      expect(rows.join("")).not.toContain("H");
    }
    expect(composeShrimp({ frame: 0, color: "#000000" })).not.toEqual(composeShrimp({ frame: 1, color: "#000000" }));
  });
});

describe("frameAt", () => {
  it("cycles at the pose's rate and stays in range", () => {
    expect(frameAt("type", 0, 0)).toBe(0);
    // type runs at 6 fps → frame 1 after 1/6 s.
    expect(frameAt("type", 170, 0)).toBe(1);
    for (let t = 0; t < 5000; t += 37) expect(frameAt("run", t, 0.3)).toBeLessThan(POSE_FRAMES.run.n);
  });

  it("offsets lobsters by phase so a crowd does not move in lockstep", () => {
    expect(frameAt("idle", 0, 0)).not.toBe(frameAt("idle", 0, 1));
  });
});
