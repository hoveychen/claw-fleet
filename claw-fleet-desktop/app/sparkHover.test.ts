import { describe, expect, it } from "vitest";
import { pickHoverPoint } from "./sparkHover";

const data = [{ v: 1 }, { v: 2 }, { v: 3 }];

describe("pickHoverPoint", () => {
  it("returns the datum at a numeric index", () => {
    expect(pickHoverPoint(data, 0)).toEqual({ v: 1 });
    expect(pickHoverPoint(data, 2)).toEqual({ v: 3 });
  });

  it("coerces the numeric-string index recharts may hand back", () => {
    expect(pickHoverPoint(data, "1")).toEqual({ v: 2 });
  });

  it("returns null when nothing is hovered", () => {
    expect(pickHoverPoint(data, undefined)).toBeNull();
    // Number(null) is 0, so a null index must not resolve to the first datum.
    expect(pickHoverPoint(data, null)).toBeNull();
  });

  it("returns null for an index past the end of the rolling window", () => {
    expect(pickHoverPoint(data, 3)).toBeNull();
    expect(pickHoverPoint(data, -1)).toBeNull();
  });

  it("returns null for a non-integer index", () => {
    expect(pickHoverPoint(data, "abc")).toBeNull();
    expect(pickHoverPoint(data, 1.5)).toBeNull();
  });

  it("returns null on empty data", () => {
    expect(pickHoverPoint([], 0)).toBeNull();
  });
});
