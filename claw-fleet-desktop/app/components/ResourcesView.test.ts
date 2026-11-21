import { describe, expect, it } from "vitest";
import type { QueuedRequest } from "../generated/types";
import { moveTargetFor } from "./ResourcesView";

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
