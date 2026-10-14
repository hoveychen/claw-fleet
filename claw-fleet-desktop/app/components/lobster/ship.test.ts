import { describe, expect, it } from "vitest";
import { composeShip, paintFor, SHIP_H, SHIP_PAINTS, SHIP_W, SHIP_WINDOWS } from "./ship";

const ship = (windows: number, thrust = false, frame = 0) =>
  composeShip({ windows: Array(windows).fill("#ffffff"), paint: "#d9452b", thrust, frame });

describe("composeShip", () => {
  it("has a fixed size", () => {
    const rows = ship(3, true);
    expect(rows).toHaveLength(SHIP_H);
    for (const r of rows) expect(r).toHaveLength(SHIP_W);
  });

  it("lights one porthole per crew member and leaves the rest dark", () => {
    const flat = ship(3).join("");
    for (const i of ["0", "1", "2"]) expect(flat).toContain(i);
    expect(flat).not.toContain("3");
    expect(flat).toContain("w");
    expect(ship(SHIP_WINDOWS).join("")).not.toContain("w");
  });

  it("caps lit portholes at the slot count", () => {
    expect(ship(SHIP_WINDOWS + 5).join("")).not.toContain(String(SHIP_WINDOWS));
  });

  it("burns the engine only under thrust, flickering between frames", () => {
    expect(ship(1).join("")).not.toMatch(/[oy]/);
    expect(ship(1, true, 0).join("")).toMatch(/o/);
    expect(ship(1, true, 0)).not.toEqual(ship(1, true, 1));
  });
});

describe("paintFor", () => {
  it("is stable per workspace and drawn from the palette", () => {
    expect(paintFor("claw-fleet")).toBe(paintFor("claw-fleet"));
    expect(SHIP_PAINTS).toContain(paintFor("anything"));
  });
});
